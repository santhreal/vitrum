//! Which agent a session is running, and the mark that says so at 16px.
//!
//! # Why this exists at all
//!
//! The tab strip is a row of names and the names do not fit. Measured at
//! `--rg-text-xs` (12px) against every face `--rg-font-ui` can resolve to on a
//! Linux desktop, the string `claude` inks 35.0px in Cantarell, 35.4 in Ubuntu,
//! 37.2 in Noto Sans and 39.9 in DejaVu Sans. The title box in the narrowest
//! tab is 27px. So at eight tabs the strip already truncates every agent name
//! to `cla…` whatever font the platform picked, and text is not a channel it
//! has.
//!
//! The case this is really for is worse than truncation. An unnamed session
//! takes the command's basename as its title
//! (`vitrum-core/src/session.rs::default_title`), but a session can be renamed
//! (`SessionManager::rename`), and a tab reading `auth refactor` carries no
//! agent identity at all, in text or in colour, at any width.
//!
//! That is the whole justification, and it is written down here because the
//! project's own rule is to prefer a word to a glyph. The word is unavailable,
//! measurably, on this surface only. On a 400px sidebar row the word does fit,
//! and a mark beside the word it depicts would be noise, which is why nothing
//! in this module is called from the sidebar.
//!
//! # What the marks are, and what they are not
//!
//! One family: a 16-unit box, a 1.25-unit stroke, `currentColor`, no fill
//! except one solid dot, round caps and joins. Pairs differ by TOPOLOGY rather
//! than by detail, because detail is the first thing to go at 16px: a ring, a
//! hexagon, a concave star, three bars, eight radial strokes, a chevron and
//! four corner brackets are told apart by outline alone, with no colour and no
//! legend.
//!
//! Two are brand likenesses: [`AgentKind::Claude`]'s radiating burst and
//! [`AgentKind::Gemini`]'s four-pointed sparkle, both of which already are line
//! geometry. Three are honest geometric tokens with no likeness claimed:
//! [`AgentKind::Codex`] takes the OpenAI mark's fold count and outer envelope
//! but not its six lobes, which cannot be drawn at this size in this weight;
//! [`AgentKind::Opencode`] and [`AgentKind::Veyyon`] have no public mark to
//! draw. A learnable token that uniquely names the agent is information. A bad
//! likeness is a lie about a brand, and worse, it is a lie the operator has to
//! squint at. Every tab names its agent in the tooltip, so the shape is
//! learnable rather than a private code.
//!
//! No letters, no digits, no monograms. The sidebar's monogram tiles were
//! removed and nothing replaced them; a
//! letter-initial avatar is a standing ban in this product.

/// Which agent is behind a session.
///
/// [`AgentKind::Unknown`] is a real answer and never a fallback dressed as one.
/// A command this build does not recognise gets the unknown mark, not the
/// nearest agent's: a wrong provider mark on a tab is a confident wrong answer,
/// and the operator has no way to tell it from a right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    Opencode,
    Veyyon,
    /// An interactive shell. Its own identity, not a failure to recognise one:
    /// a `bash` tab is a shell on purpose and the operator knows it.
    Shell,
    /// Nothing this build recognises.
    Unknown,
}

/// The agent binaries this build knows, keyed on the command rather than on the
/// label.
///
/// The commands are exactly `launch.rs`'s `AGENTS` table, so a session started
/// from the picker and one typed into the command field resolve to the same
/// mark. The labels are that table's too, and they are what the tooltip says.
const AGENTS: [(&str, AgentKind, &str); 5] = [
    ("claude", AgentKind::Claude, "Claude Code"),
    ("codex", AgentKind::Codex, "Codex"),
    ("gemini", AgentKind::Gemini, "Gemini CLI"),
    ("opencode", AgentKind::Opencode, "opencode"),
    ("veyyon", AgentKind::Veyyon, "veyyon"),
];

/// Interactive shells, which get the prompt mark rather than the unknown one.
///
/// Login shells reach this from `Options::shell` and from history entries like
/// `/bin/bash`, so the common case on a fresh machine is a shell and not an
/// agent. Reporting that as unknown would put a dashed placeholder on the one
/// kind of session every operator has.
const SHELLS: [&str; 15] = [
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "mksh",
    "csh",
    "tcsh",
    "nu",
    "elvish",
    "xonsh",
    "pwsh",
    "powershell",
    "cmd",
];

/// Suffixes `PATHEXT` resolution can leave on a Windows command.
///
/// `launch.rs::on_path` honours `PATHEXT`, so `claude` can resolve to
/// `claude.cmd` and arrive here with the extension attached. Without this a
/// Windows operator would see the unknown mark on every agent tab.
const EXECUTABLE_SUFFIXES: [&str; 5] = [".exe", ".cmd", ".bat", ".com", ".ps1"];

impl AgentKind {
    /// Resolve the agent behind a command line's program.
    ///
    /// The program's basename is lowercased and stripped of a Windows
    /// executable suffix, then matched exactly. There is no prefix match and no
    /// nearest neighbour: `claudex` and `my-claude` are not Claude Code, and
    /// guessing that they are would put the wrong provider on a tab with no way
    /// for the operator to notice.
    pub fn of(command: &str) -> Self {
        // Split on BOTH separators rather than through `std::path`. `Path` uses
        // the host's rules, so on Linux `C:\tools\codex.exe` has no components
        // at all and resolves to unknown, and on Windows a forward-slash path
        // does resolve but only by accident of that platform accepting both.
        // The command string arrives from `SessionInfo` exactly as the operator
        // or the launcher wrote it, and both forms are ordinary.
        let base = command.rsplit(['/', '\\']).next().unwrap_or(command);
        let lower = base.to_ascii_lowercase();
        let name = EXECUTABLE_SUFFIXES
            .iter()
            .find_map(|suffix| lower.strip_suffix(suffix))
            .unwrap_or(lower.as_str());

        if let Some((_, kind, _)) = AGENTS.iter().find(|(cmd, _, _)| *cmd == name) {
            return *kind;
        }
        if SHELLS.contains(&name) {
            return AgentKind::Shell;
        }
        AgentKind::Unknown
    }

    /// The operator-facing name, for a tooltip.
    ///
    /// The five agent labels come out of [`AGENTS`] rather than a second match,
    /// so the picker that started the session and the tab that hosts it cannot
    /// end up calling it two different things. The two remaining kinds are not
    /// in that table because they are not binaries this build looks for.
    ///
    /// The `unwrap_or` is total rather than a panic and is never taken while
    /// [`AGENTS`] names all five agent variants, which
    /// `every_agent_has_its_own_mark_and_its_own_label` proves.
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Shell => "shell",
            AgentKind::Unknown => "unknown agent",
            agent => AGENTS
                .iter()
                .find(|(_, kind, _)| *kind == agent)
                .map(|(_, _, label)| *label)
                .unwrap_or("agent"),
        }
    }

    /// The drawn mark.
    ///
    /// Coordinates are in the 16-unit box every mark shares. Optical size, not
    /// bounding box, is what is held constant: a shape reads smaller as it
    /// encloses less of its box, so the ring is 11.0 across, the straight-sided
    /// marks are 9.5 to 10.2, and the two radial marks run 13.6 tip to tip. A
    /// 13.6 asterisk and a 10.2 hexagon look the same size; two 13.6 boxes do
    /// not. Weight is equalised the other way, by one stroke width for all
    /// seven, so no mark reads bolder than its neighbour.
    ///
    /// Every mark is symmetric about y = 8, so flex centring lands its optical
    /// centre on the title's without a per-mark nudge.
    pub fn mark(self) -> AgentMark {
        match self {
            // Eight spokes from an open centre, r 2.2 to 6.8: 13.6 tip to tip
            // with a 4.4 hole in the middle. Anthropic's radiating mark, and
            // the only thing to resolve at 16px is direction.
            AgentKind::Claude => AgentMark {
                stroke: "M8 1.2V5.8M8 10.2V14.8M1.2 8H5.8M10.2 8H14.8\
                         M3.19 3.19L6.44 6.44M9.56 9.56L12.81 12.81\
                         M12.81 3.19L9.56 6.44M6.44 9.56L3.19 12.81",
                fill: "",
            },
            // Pointy-top hexagon, circumradius 5.9, so 10.22 across the flats.
            // Six-fold like the OpenAI mark and its outer envelope; the six
            // lobes are not attempted, because at a 1.25 stroke in 16px they
            // close into a blob.
            AgentKind::Codex => AgentMark {
                stroke: "M8 2.1L13.11 5.05L13.11 10.95L8 13.9L2.89 10.95L2.89 5.05Z",
                fill: "",
            },
            // Four-pointed sparkle, tips at 6.8 on the axes, waists at 1.91.
            // The waist is 1.5x the stroke, which is what keeps it from
            // filling in. Google's Gemini mark.
            AgentKind::Gemini => AgentMark {
                stroke: "M8 1.2Q9.35 6.65 14.8 8Q9.35 9.35 8 14.8\
                         Q6.65 9.35 1.2 8Q6.65 6.65 8 1.2Z",
                fill: "",
            },
            // Three left-aligned bars, 9.5 / 6.5 / 4. The only mark in the set
            // built from horizontals alone, which is what tells it apart at a
            // glance. No public mark exists; this is a token, and the thing it
            // depicts is code.
            AgentKind::Opencode => AgentMark {
                stroke: "M3.25 4.5H12.75M3.25 8H9.75M3.25 11.5H7.25",
                fill: "",
            },
            // An 11.0 ring around a 3.0 dot, 3.4 of clear annulus between
            // them. No public mark exists; this is a token, and it is the only
            // mark with a solid element, which is what makes it unmistakable.
            AgentKind::Veyyon => AgentMark {
                stroke: "M2.5 8a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0-11 0",
                fill: "M6.5 8a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0",
            },
            // A prompt: chevron plus the cursor bar under it. Depicts what a
            // shell is rather than who wrote it, and it is the one mark in the
            // set every terminal user already knows.
            AgentKind::Shell => AgentMark {
                stroke: "M3.4 4.6L7 8L3.4 11.4M8.8 11.4H12.8",
                fill: "",
            },
            // The four corners of a 10.0 square with 4.0 missing from the
            // middle of every side: the universal placeholder for "nothing
            // identified". Four disjoint strokes, so it cannot be mistaken for
            // any closed mark above, and it says unknown without a letter.
            AgentKind::Unknown => AgentMark {
                stroke: "M3 6V4.5A1.5 1.5 0 0 1 4.5 3H6M10 3H11.5A1.5 1.5 0 0 1 13 4.5V6\
                         M13 10V11.5A1.5 1.5 0 0 1 11.5 13H10M6 13H4.5A1.5 1.5 0 0 1 3 11.5V10",
                fill: "",
            },
        }
    }
}

/// One mark's path data, in the shared 16-unit box.
///
/// Two subpaths at most, because every extra element is a node on a surface
/// that repaints on every daemon frame. `fill` is empty for six of the seven
/// marks and carries [`AgentKind::Veyyon`]'s centre dot for the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentMark {
    /// Stroked subpaths: `fill: none`, `stroke: currentColor`.
    pub stroke: &'static str,
    /// Solid subpaths, or `""` when the mark has none.
    pub fill: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind, so a variant added without a mark, a label or a test fails
    /// here rather than rendering blank in the strip.
    const ALL: [AgentKind; 7] = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Gemini,
        AgentKind::Opencode,
        AgentKind::Veyyon,
        AgentKind::Shell,
        AgentKind::Unknown,
    ];

    /// Position of one kind, as an EXHAUSTIVE match.
    ///
    /// This is the chain that makes [`ALL`] enforced rather than trusted. Rust
    /// cannot enumerate an enum's variants, so a hand-kept list is the only
    /// option and a hand-kept list of the thing a guard names is a hole: three
    /// tests iterate `ALL`, and an eighth agent left out of it would be checked
    /// by none of them while shipping a mark nobody had looked at.
    ///
    /// Adding a variant breaks this match, which stops the crate compiling.
    /// Giving it the next index then overruns `seen` below, which panics. Sizing
    /// `seen` to fit then leaves a `false` in it until `ALL` is updated too. So
    /// the only way to a green build is to add the variant everywhere.
    fn index(kind: AgentKind) -> usize {
        match kind {
            AgentKind::Claude => 0,
            AgentKind::Codex => 1,
            AgentKind::Gemini => 2,
            AgentKind::Opencode => 3,
            AgentKind::Veyyon => 4,
            AgentKind::Shell => 5,
            AgentKind::Unknown => 6,
        }
    }

    /// [`ALL`] must name every variant exactly once.
    ///
    /// THE BUG, and it is LauncherRedesign's shape: a guard that polices a
    /// hand-kept list of the thing it names. Adding an agent is the most likely
    /// future change to this file, and an agent missing from `ALL` is invisible
    /// to `every_agent_has_its_own_mark_and_its_own_label`,
    /// `only_the_veyyon_ring_has_a_solid_element` and
    /// `no_mark_reaches_outside_its_box` at once.
    #[test]
    fn the_all_list_names_every_variant_exactly_once() {
        let mut seen = [0usize; 7];
        for kind in ALL {
            seen[index(kind)] += 1;
        }
        assert_eq!(
            seen, [1; 7],
            "ALL must name each variant once; index {seen:?} counted per variant"
        );
    }

    /// The five commands `launch.rs` offers must each reach their own kind.
    ///
    /// The bug: the picker starts `gemini` and the tab draws Claude's burst,
    /// because the table here drifted from `launch.rs`'s. The operator then has
    /// no way at all to tell which agent a tab is, since the mark is the only
    /// channel that says so on a renamed session.
    #[test]
    fn the_launcher_commands_each_resolve_to_their_own_agent() {
        assert_eq!(AgentKind::of("claude"), AgentKind::Claude);
        assert_eq!(AgentKind::of("codex"), AgentKind::Codex);
        assert_eq!(AgentKind::of("gemini"), AgentKind::Gemini);
        assert_eq!(AgentKind::of("opencode"), AgentKind::Opencode);
        assert_eq!(AgentKind::of("veyyon"), AgentKind::Veyyon);
    }

    /// A full path, an uppercased name and a `PATHEXT` suffix must all still
    /// resolve.
    ///
    /// The bug: `SessionInfo::command` holds whatever was typed or resolved, so
    /// `/home/me/.local/bin/claude` and `claude.cmd` are both ordinary. Matching
    /// the raw string puts the unknown placeholder on every agent tab on
    /// Windows, and on any machine where the operator typed a path.
    #[test]
    fn a_path_a_case_and_a_windows_suffix_still_resolve() {
        assert_eq!(
            AgentKind::of("/home/me/.local/bin/claude"),
            AgentKind::Claude
        );
        assert_eq!(AgentKind::of("CLAUDE"), AgentKind::Claude);
        assert_eq!(AgentKind::of("claude.cmd"), AgentKind::Claude);
        assert_eq!(AgentKind::of(r"C:\tools\codex.exe"), AgentKind::Codex);
        assert_eq!(AgentKind::of("/usr/bin/bash"), AgentKind::Shell);
        assert_eq!(AgentKind::of("PowerShell.EXE"), AgentKind::Shell);
    }

    /// An unrecognised command must report unknown, never the nearest agent.
    ///
    /// The bug: a prefix or substring match. `claudex`, `my-claude` and
    /// `claude-wrapper` all contain `claude`, and a tab that draws Anthropic's
    /// burst for someone else's binary is a confident wrong answer the operator
    /// cannot detect. `env` is the live case: `/usr/bin/env sh -c ...` is a
    /// perfectly ordinary command line whose program identifies nothing.
    #[test]
    fn an_unrecognised_command_is_unknown_and_not_the_nearest_agent() {
        for command in [
            "",
            "env",
            "/usr/bin/env",
            "claudex",
            "my-claude",
            "claude-wrapper",
            "gemini2",
            "shell",
            "make",
        ] {
            assert_eq!(
                AgentKind::of(command),
                AgentKind::Unknown,
                "{command:?} must be unknown rather than guessed"
            );
        }
        assert_eq!(AgentKind::of("env").mark(), AgentKind::Unknown.mark());
        assert_ne!(AgentKind::of("env").mark(), AgentKind::Claude.mark());
    }

    /// Seven kinds, seven different marks and seven different labels.
    ///
    /// The bug this locks out is a copy-paste in the match above leaving two
    /// agents with the same path data. Two identical marks make the icon worse
    /// than absent: it looks like it is answering the question and is not.
    #[test]
    fn every_agent_has_its_own_mark_and_its_own_label() {
        let marks: Vec<AgentMark> = ALL.iter().map(|k| k.mark()).collect();
        for (i, a) in marks.iter().enumerate() {
            assert!(!a.stroke.is_empty(), "{:?} has no stroked path", ALL[i]);
            for (j, b) in marks.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "{:?} and {:?} draw the same mark", ALL[i], ALL[j]);
            }
        }
        let labels: Vec<&str> = ALL.iter().map(|k| k.label()).collect();
        assert_eq!(
            labels,
            vec![
                "Claude Code",
                "Codex",
                "Gemini CLI",
                "opencode",
                "veyyon",
                "shell",
                "unknown agent",
            ]
        );
    }

    /// One mark carries a solid element and the other six do not.
    ///
    /// The bug: a stray `fill` on a stroked mark, which at 16px turns an
    /// outline into a blob and destroys the topological distinctness the whole
    /// set rests on.
    #[test]
    fn only_the_veyyon_ring_has_a_solid_element() {
        for kind in ALL {
            let filled = !kind.mark().fill.is_empty();
            assert_eq!(
                filled,
                kind == AgentKind::Veyyon,
                "{kind:?} solid element: {filled}"
            );
        }
    }

    /// A mark drawn from absolute coordinates alone must fill the envelope its
    /// shape class shares.
    ///
    /// THE BUG, found by mutation rather than by reading: a mark retuned to a
    /// different size. A coordinate moved WITHIN the box passes every other
    /// guard in this file. A burst shrunk from 13.6 tip to tip to 10.0 stays
    /// inside `no_mark_reaches_outside_its_box`, stays distinct under
    /// `every_agent_has_its_own_mark_and_its_own_label`, keeps its lack of a
    /// fill, and renders visibly smaller than its neighbours with nothing at
    /// all to catch it. [`AgentKind::mark`] states the envelope per class in
    /// prose and, until this, nothing checked it, which is a claim with no test
    /// behind it.
    ///
    /// SCOPED to the marks whose data is `M`/`L`/`V`/`H`/`Q` only, where every
    /// number is a coordinate. Only Unknown and Veyyon are excluded: Unknown
    /// carries `A` radii and sweep flags and Veyyon's arcs are relative, so
    /// their extent needs a real path parser, and a hand-rolled parser inside a
    /// test is a new failure surface guarding a hypothetical. Those two are
    /// named here as unchecked rather than approximated.
    ///
    /// An earlier revision of this comment claimed Codex was excluded too, on
    /// the false premise that its hexagon carried arcs. It is pure `L`, so its
    /// size was going unchecked for no reason at all. The completeness clause
    /// below caught that on its first run, which is the whole argument for
    /// deriving a guard's scope from the data instead of restating it in prose.
    /// The precondition is asserted, not assumed: each declared kind must carry
    /// no arc command.
    #[test]
    fn every_absolute_mark_fills_the_optical_envelope() {
        // This metric measures to the furthest POINT, so radial marks give tip
        // to tip and a closed polygon gives vertex to vertex. Codex's 11.8 is
        // its point-to-point diameter; the 10.22 across its flats is the figure
        // `mark` quotes for optical size, and the two are the same hexagon.
        // The band cannot be derived from the data, so it is declared per kind
        // and the table's COMPLETENESS is enforced below instead. Asserting only
        // "in one of the bands" would be derivable and would also be useless:
        // 10.0 sits inside the straight band, so the shrunk burst this guard
        // exists to catch would pass.
        let envelopes = [
            (AgentKind::Claude, 13.4, 13.8),
            (AgentKind::Gemini, 13.4, 13.8),
            (AgentKind::Codex, 11.6, 12.0),
            (AgentKind::Opencode, 9.4, 10.4),
            (AgentKind::Shell, 9.4, 10.4),
        ];
        for (kind, low, high) in envelopes {
            let data = kind.mark().stroke;
            assert!(
                !data.contains('A') && !data.contains('a'),
                "{kind:?} carries an arc, so its numbers are not all coordinates \
                 and this extent would be wrong rather than merely unchecked"
            );
            let extent = numbers(data)
                .into_iter()
                .map(|n| (n - 8.0).abs())
                .fold(0.0f64, f64::max)
                * 2.0;
            assert!(
                (low..=high).contains(&extent),
                "{kind:?} spans {extent} against the {low} to {high} envelope \
                 its class shares, so it will read as a different size"
            );
        }

        // COMPLETENESS, so the table above cannot be the hand-kept list that
        // silently omits a new mark. Every arc-free kind in `ALL` must be in it,
        // and only arc-carrying kinds may be absent.
        for kind in ALL {
            let stroke = kind.mark().stroke;
            let arc_free = !stroke.contains('A') && !stroke.contains('a');
            let declared = envelopes.iter().any(|(k, _, _)| *k == kind);
            assert_eq!(
                declared, arc_free,
                "{kind:?} is arc-free: {arc_free}, declared in the envelope \
                 table: {declared}. An arc-free mark with no envelope is a mark \
                 whose size nothing checks."
            );
        }
    }

    /// Every coordinate in every mark sits inside the 16-unit box.
    ///
    /// The bug: a typo like `18` for `1.8` puts part of a glyph outside the
    /// viewBox, where SVG clips it. The result is a mark missing a stroke,
    /// which is indistinguishable from a different agent's mark and fails no
    /// other test. The margin is the stroke's own half-width plus a round cap,
    /// 0.625 either side, so ink stays inside the box too.
    #[test]
    fn no_mark_reaches_outside_its_box() {
        for kind in ALL {
            let mark = kind.mark();
            for data in [mark.stroke, mark.fill] {
                for number in numbers(data) {
                    assert!(
                        number.abs() <= 16.625,
                        "{kind:?} has {number} in {data:?}, outside the 16-unit box"
                    );
                }
            }
        }
    }

    /// Pull every numeric literal out of SVG path data.
    ///
    /// Relative commands take negative arguments (`-11` closes the ring's
    /// second arc), so a minus sign starts a number; `a`/`A` arc segments carry
    /// radii and flags, which are all inside the box's range anyway.
    fn numbers(data: &str) -> Vec<f64> {
        let mut out = Vec::new();
        let mut token = String::new();
        let flush = |token: &mut String, out: &mut Vec<f64>| {
            if let Ok(n) = token.parse::<f64>() {
                out.push(n);
            }
            token.clear();
        };
        for c in data.chars() {
            if c.is_ascii_digit() || c == '.' {
                token.push(c);
            } else if c == '-' {
                // A minus both ENDS the number before it and starts the next:
                // `0-11` is two arguments, and a scanner that only treated it
                // as a terminator dropped the sign and reported +11.
                flush(&mut token, &mut out);
                token.push(c);
            } else {
                flush(&mut token, &mut out);
            }
        }
        flush(&mut token, &mut out);
        out
    }

    /// The extractor must actually find numbers.
    ///
    /// The bug: a parser that silently yields nothing makes
    /// `no_mark_reaches_outside_its_box` a test that asserts against an empty
    /// list and passes on any input at all.
    #[test]
    fn the_coordinate_extractor_reads_real_values() {
        assert_eq!(numbers("M8 1.2V5.8"), vec![8.0, 1.2, 5.8]);
        assert_eq!(
            numbers("a5.5 5.5 0 1 0-11 0"),
            vec![5.5, 5.5, 0.0, 1.0, 0.0, -11.0, 0.0]
        );
        assert!(numbers(AgentKind::Claude.mark().stroke).len() >= 24);
        assert!(numbers("").is_empty());
    }
}
