//! WHY: "the palette ignores my terminal" is a real gap, and the way an import
//! fails it is by looking as if it worked. A parser that reads twelve of
//! sixteen slots produces a grid where four colours are black; a parser that
//! zero-extends `#fff` produces a dark grey where the operator wrote white; a
//! parser that takes the first candidate it can open produces the colours of a
//! terminal the operator does not run.
//!
//! So the rules asserted here are: an incomplete import is refused rather than
//! merged, a colour is scaled and not truncated, the candidate order follows
//! what the environment says about itself, and every format that ships is
//! actually read.
//!
//! What this does NOT catch: whether the file the scan found belongs to the
//! terminal the operator is looking at. Nothing can, and the import records
//! the file it read so the answer is visible rather than assumed.

use super::*;

/// A whole palette in the flat form, as a fixture.
const FLAT: &str = "\
# a comment
background #101010
foreground #d0d0d0
cursor     #ffcc00
color0  #000000
color1  #cc0000
color2  #00cc00
color3  #cccc00
color4  #0000cc
color5  #cc00cc
color6  #00cccc
color7  #cccccc
color8  #666666
color9  #ff0000
color10 #00ff00
color11 #ffff00
color12 #0000ff
color13 #ff00ff
color14 #00ffff
color15 #ffffff
";

/// The same palette in the sectioned form that names its colours.
const SECTIONED: &str = r##"
[colors.primary]
background = "#101010"
foreground = "#d0d0d0"

[colors.cursor]
cursor = "#ffcc00"

[colors.normal]
black   = "#000000"
red     = "#cc0000"
green   = "#00cc00"
yellow  = "#cccc00"
blue    = "#0000cc"
magenta = "#cc00cc"
cyan    = "#00cccc"
white   = "#cccccc"

[colors.bright]
black   = "#666666"
red     = "#ff0000"
green   = "#00ff00"
yellow  = "#ffff00"
blue    = "#0000ff"
magenta = "#ff00ff"
cyan    = "#00ffff"
white   = "#ffffff"
"##;

/// The same palette in the sectioned form that numbers its colours.
const SECTIONED_NUMBERED: &str = "\
[colors]
background=101010
foreground=d0d0d0
regular0=000000
regular1=cc0000
regular2=00cc00
regular3=cccc00
regular4=0000cc
regular5=cc00cc
regular6=00cccc
regular7=cccccc
bright0=666666
bright1=ff0000
bright2=00ff00
bright3=ffff00
bright4=0000ff
bright5=ff00ff
bright6=00ffff
bright7=ffffff
";

/// The same palette as X resources, with a class prefix and a comment.
const X_RESOURCES: &str = "\
! a comment
*background: #101010
*foreground: #d0d0d0
*cursorColor: #ffcc00
*color0: #000000
*color1: rgb:cc/00/00
*color2: #00cc00
*color3: #cccc00
*color4: #0000cc
*color5: #cc00cc
*color6: #00cccc
*color7: #cccccc
*color8: #666666
*color9: #ff0000
*color10: #00ff00
*color11: #ffff00
*color12: #0000ff
*color13: #ff00ff
*color14: #00ffff
*color15: #ffffff
";

/// The same palette as a JSON scheme list.
const JSON: &str = r##"{
  "schemes": [{
    "name": "Fixture",
    "background": "#101010", "foreground": "#d0d0d0", "cursorColor": "#ffcc00",
    "black": "#000000", "red": "#cc0000", "green": "#00cc00", "yellow": "#cccc00",
    "blue": "#0000cc", "magenta": "#cc00cc", "cyan": "#00cccc", "white": "#cccccc",
    "brightBlack": "#666666", "brightRed": "#ff0000", "brightGreen": "#00ff00",
    "brightYellow": "#ffff00", "brightBlue": "#0000ff", "brightMagenta": "#ff00ff",
    "brightCyan": "#00ffff", "brightWhite": "#ffffff"
  }]
}"##;

/// Every format ships and every format is read, to the same twenty colours.
///
/// THE BUG this stops: a format listed in [`candidates`] with no parser behind
/// it, which is an operator whose terminal the product claims to support and
/// silently does not.
#[test]
fn every_format_reads_the_same_palette() {
    for (name, text, format) in [
        ("flat", FLAT, HostSource::Flat),
        ("sectioned by name", SECTIONED, HostSource::Sectioned),
        (
            "sectioned by number",
            SECTIONED_NUMBERED,
            HostSource::Sectioned,
        ),
        ("x resources", X_RESOURCES, HostSource::XResources),
        ("json", JSON, HostSource::Json),
    ] {
        let p = parse(text, format);
        assert!(p.is_complete(), "{name} did not yield a whole palette: {p:?}");
        assert_eq!(p.background, "#101010", "{name}");
        assert_eq!(p.foreground, "#d0d0d0", "{name}");
        assert_eq!(p.ansi[0], "#000000", "{name}");
        assert_eq!(p.ansi[1], "#cc0000", "{name}");
        assert_eq!(p.ansi[8], "#666666", "{name}");
        assert_eq!(p.ansi[15], "#ffffff", "{name}");
        assert_eq!(p.source, format, "{name}");
    }
}

/// Every format that declares a cursor yields it, and one that does not falls
/// back rather than yielding black.
#[test]
fn a_missing_cursor_falls_back_to_the_foreground() {
    let with = parse(FLAT, HostSource::Flat);
    assert_eq!(with.cursor_or_foreground(), "#ffcc00");

    let without = parse(SECTIONED_NUMBERED, HostSource::Sectioned);
    assert!(without.cursor.is_empty());
    assert_eq!(without.cursor_or_foreground(), "#d0d0d0");
    assert_eq!(without.selection_or_foreground(), "#d0d0d0");
}

/// An import short of a slot is refused, and says which slot.
///
/// THE BUG this stops: a partial read applied anyway, which paints the missing
/// slots black. On a dark scheme that is invisible until an agent prints in
/// that colour and the line disappears.
#[test]
fn an_incomplete_file_is_refused_by_name() {
    let short = FLAT.replace("color7  #cccccc\n", "");
    let p = parse(&short, HostSource::Flat);
    assert!(!p.is_complete());
    assert_eq!(missing_of(&p), "no colour 7");

    let no_background = FLAT.replace("background #101010\n", "");
    let p = parse(&no_background, HostSource::Flat);
    assert!(!p.is_complete());
    assert_eq!(missing_of(&p), "no background");
}

/// A file that declares nothing this product understands is not an import.
#[test]
fn a_file_with_no_colours_is_not_an_import() {
    let p = parse("shell zsh\nfont_size 12\n", HostSource::Flat);
    assert_eq!(p.source, HostSource::None);
    assert!(!p.is_complete());
    assert_eq!(missing_of(&p), "no background, no foreground, no ANSI colours");
}

/// A short colour is scaled to full range, not zero-extended.
///
/// THE BUG this stops, and it is the classic one: `#fff` read as `#0f0f0f`.
/// The operator wrote white and the grid painted near-black, and every
/// individual colour still parsed, so nothing reported a failure.
#[test]
fn a_short_colour_is_scaled_and_not_zero_extended() {
    assert_eq!(parse_colour("#fff").as_deref(), Some("#ffffff"));
    assert_eq!(parse_colour("#000").as_deref(), Some("#000000"));
    assert_eq!(parse_colour("#f00").as_deref(), Some("#ff0000"));
    assert_eq!(parse_colour("#ffffffffffff").as_deref(), Some("#ffffff"));
    assert_eq!(parse_colour("rgb:ff/00/00").as_deref(), Some("#ff0000"));
    assert_eq!(parse_colour("rgb:f/f/f").as_deref(), Some("#ffffff"));
    assert_eq!(parse_colour("rgb:ffff/0000/0000").as_deref(), Some("#ff0000"));
}

/// Every syntax the files use is accepted, and nothing else is.
#[test]
fn only_a_colour_parses_as_a_colour() {
    for good in [
        "#102030",
        "  \"#102030\"  ",
        "'#102030'",
        "0x102030",
        "#10203040",
    ] {
        assert_eq!(
            parse_colour(good).as_deref(),
            Some("#102030"),
            "{good} did not parse"
        );
    }
    for bad in [
        "",
        "none",
        "#12345",
        "#gggggg",
        "102030",
        "rgb:ff/00",
        "rgb:ff/00/00/00",
        "; }",
        "url(x)",
    ] {
        assert_eq!(parse_colour(bad), None, "{bad} parsed as a colour");
    }
}

/// A hand-edited profile cannot put a non-colour into the renderer.
///
/// THE BUG this stops: an operator editing the stored import by hand, or a
/// profile written by a newer build with a colour syntax this one does not
/// read. The clamp empties the slot, which makes the palette incomplete, which
/// takes the switch out of force rather than painting whatever the string was.
#[test]
fn a_hand_edited_import_is_refused_rather_than_painted() {
    let mut p = parse(FLAT, HostSource::Flat);
    assert!(p.is_complete());
    p.ansi[3] = "; }".to_string();
    p.background = "expression(alert)".to_string();
    p.clamp();
    assert_eq!(p.ansi[3], "");
    assert_eq!(p.background, "");
    assert!(
        !p.is_complete(),
        "a palette with a rejected slot must not be in force"
    );
}

/// An oversized `ansi` array is cut to sixteen.
#[test]
fn an_oversized_import_is_cut_to_the_sixteen_slots() {
    let mut p = parse(FLAT, HostSource::Flat);
    p.ansi.extend(std::iter::repeat_n("#123456".to_string(), 400));
    p.clamp();
    assert_eq!(p.ansi.len(), 16);
    assert!(p.is_complete());
}

/// The environment decides which candidate is asked first.
///
/// THE BUG this stops: an operator running one terminal and having the
/// configuration of another installed, and getting the other one's colours
/// with no signal that it happened.
#[test]
fn the_terminal_that_names_itself_is_asked_first() {
    let base: BTreeMap<String, String> = [("HOME".to_string(), "/home/mk".to_string())]
        .into_iter()
        .collect();
    let first = |env: &BTreeMap<String, String>| -> String {
        candidates(env)[0]
            .path
            .file_name()
            .expect("a candidate has a file name")
            .to_string_lossy()
            .to_string()
    };

    assert_eq!(first(&base), "alacritty.toml", "the shipped order moved");

    for (key, value, want) in [
        ("KITTY_WINDOW_ID", "1", "kitty.conf"),
        ("ALACRITTY_WINDOW_ID", "1", "alacritty.toml"),
        ("TERM", "foot-extra", "foot.ini"),
        ("WT_SESSION", "abc", "settings.json"),
    ] {
        let mut env = base.clone();
        env.insert(key.to_string(), value.to_string());
        assert_eq!(first(&env), want, "{key}={value}");
    }
}

/// Every candidate is still offered, whatever the environment says.
///
/// THE BUG this stops: preferring one terminal by trimming the list to it, so
/// an operator whose terminal exports a variable and keeps its colours
/// somewhere else gets "no terminal configuration was found" while one is on
/// disk.
#[test]
fn naming_one_terminal_does_not_drop_the_others() {
    let base: BTreeMap<String, String> = [("HOME".to_string(), "/home/mk".to_string())]
        .into_iter()
        .collect();
    let mut named = base.clone();
    named.insert("KITTY_WINDOW_ID".to_string(), "1".to_string());

    let plain: Vec<PathBuf> = candidates(&base).into_iter().map(|c| c.path).collect();
    let mut reordered: Vec<PathBuf> = candidates(&named).into_iter().map(|c| c.path).collect();
    reordered.sort();
    let mut sorted = plain.clone();
    sorted.sort();
    assert_eq!(reordered, sorted, "naming a terminal changed the candidate set");
    assert!(plain.len() >= 5, "{plain:?}");
}

/// The config directory follows the environment rather than the home
/// directory when the environment names one.
#[test]
fn the_config_directory_is_read_from_the_environment() {
    let env: BTreeMap<String, String> = [
        ("HOME".to_string(), "/home/mk".to_string()),
        ("XDG_CONFIG_HOME".to_string(), "/src/cfg".to_string()),
    ]
    .into_iter()
    .collect();
    let paths: Vec<PathBuf> = candidates(&env).into_iter().map(|c| c.path).collect();
    // By component, not by prefix string: a separator is `\` on one of the
    // platforms this suite runs on, and `starts_with` over the rendered text
    // would be asserting the separator rather than the directory.
    assert!(
        paths.iter().any(|p| p.starts_with("/src/cfg")),
        "{paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("/home/mk/.config")),
        "{paths:?}"
    );
}

/// The scan takes the first candidate that is whole, and skips one that is
/// not, rather than merging the two.
///
/// THE BUG this stops: a palette half from one terminal and half from another,
/// which is a set of colours nobody has ever looked at and which nothing on
/// screen would explain.
#[test]
fn a_partial_file_is_skipped_rather_than_merged() {
    let env: BTreeMap<String, String> = [("HOME".to_string(), "/home/mk".to_string())]
        .into_iter()
        .collect();
    let partial = "background #ff0000\nforeground #ff0000\n";
    let got = import(&env, |path| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match name.as_ref() {
            "alacritty.toml" => Ok(partial.to_string()),
            "kitty.conf" => Ok(FLAT.to_string()),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not here",
            )),
        }
    })
    .expect("the whole file answers");
    assert_eq!(got.background, "#101010", "the partial file was merged in");
    assert!(got.origin.ends_with("kitty.conf"), "{}", got.origin);
    assert!(got.is_complete());
}

/// Nothing on disk is a named refusal with an instruction, not an empty
/// palette.
#[test]
fn no_candidate_says_so_and_says_what_to_do() {
    let env: BTreeMap<String, String> = [("HOME".to_string(), "/home/mk".to_string())]
        .into_iter()
        .collect();
    let err = import(&env, |_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "not here",
        ))
    })
    .expect_err("nothing is installed");
    assert_eq!(err, ImportError::NoCandidate);
    let message = err.to_string();
    assert!(message.contains("Pick a built-in palette"), "{message}");
}

/// Files that all fall short report which ones and what each was missing.
#[test]
fn every_file_falling_short_reports_which_slot_it_lacked() {
    let env: BTreeMap<String, String> = [("HOME".to_string(), "/home/mk".to_string())]
        .into_iter()
        .collect();
    let short = FLAT.replace("color9  #ff0000\n", "");
    let err = import(&env, |path| {
        if path.file_name().is_some_and(|f| f == "kitty.conf") {
            Ok(short.clone())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "not here",
            ))
        }
    })
    .expect_err("no file is whole");
    let message = err.to_string();
    assert!(message.contains("kitty.conf"), "{message}");
    assert!(message.contains("no colour 9"), "{message}");
    assert!(message.contains("Declare the missing colours"), "{message}");
}

/// A read that fails for a reason other than absence is reported, not skipped.
///
/// THE BUG this stops: a permissions problem reading as "you have no terminal
/// configuration", which sends the operator looking for a file that is right
/// there.
#[test]
fn a_read_that_fails_is_reported_rather_than_skipped() {
    let env: BTreeMap<String, String> = [("HOME".to_string(), "/home/mk".to_string())]
        .into_iter()
        .collect();
    let err = import(&env, |_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ))
    })
    .expect_err("a refused read is an error");
    let message = err.to_string();
    assert!(matches!(err, ImportError::Unreadable { .. }), "{err:?}");
    assert!(message.contains("permissions"), "{message}");
}

/// A named file is read whatever format it turns out to be.
#[test]
fn a_named_file_is_read_by_shape_and_not_by_name() {
    for text in [FLAT, SECTIONED, X_RESOURCES, JSON] {
        let got = import_file(Path::new("/src/colours.conf"), |_| Ok(text.to_string()))
            .expect("the fixture is a whole palette");
        assert!(got.is_complete());
        assert_eq!(got.origin, "/src/colours.conf");
    }
}

/// A comment cannot swallow a colour, and a colour cannot start a comment.
///
/// THE BUG this stops: `#` is both the comment character in two of these
/// formats and the first character of every colour in all of them. A naive
/// split on `#` reads `color0 #000000` as `color0` with no value.
#[test]
fn the_comment_rule_does_not_eat_the_colour() {
    let p = parse(
        "color0 #000000 # the darkest one\n# a whole comment line\ncolor1 #cc0000\n",
        HostSource::Flat,
    );
    assert_eq!(p.ansi[0], "#000000");
    assert_eq!(p.ansi[1], "#cc0000");

    let sectioned = parse(
        "[colors.normal]\nblack = \"#000000\" # dark\n",
        HostSource::Sectioned,
    );
    assert_eq!(sectioned.ansi[0], "#000000");
}

/// A slot number outside the sixteen is ignored rather than panicking.
#[test]
fn a_slot_out_of_range_is_ignored() {
    let p = parse("color200 #ff0000\nbright99 #00ff00\n", HostSource::Flat);
    assert_eq!(p.source, HostSource::None);
    assert!(p.ansi.iter().all(String::is_empty));
}

/// The four bytes handed to the renderer are the colour that was imported.
#[test]
fn the_renderer_gets_the_colour_that_was_imported() {
    assert_eq!(to_rgba("#102030", 255), Some([0x10, 0x20, 0x30, 255]));
    assert_eq!(to_rgba("#102030", 128), Some([0x10, 0x20, 0x30, 128]));
    assert_eq!(to_rgba("not a colour", 255), None);
}

/// A source has a label, so a row can say where an import came from.
#[test]
fn every_source_has_a_label() {
    for source in [
        HostSource::None,
        HostSource::Sectioned,
        HostSource::Flat,
        HostSource::XResources,
        HostSource::Json,
    ] {
        assert!(!source.label().is_empty(), "{source:?}");
    }
}
