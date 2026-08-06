//! The icons an operator can put on a saved command.
//!
//! A closed set, drawn inline. No icon font, no sprite sheet, no network
//! request and no emoji: the shell is loaded from disk and an icon that needs
//! a download is an icon that is missing on the machine without one, and an
//! emoji is whatever the platform's font decided this year.
//!
//! Same family as [`crate::agent`]'s marks so the two never look like two
//! products: a 16-unit box, a 1.25-unit stroke, `currentColor`, round caps and
//! joins, one optional solid subpath. Those marks say which agent a session is
//! running and are chosen by the program name; these are chosen by the
//! operator, which is why they are a separate table with its own slugs.
//!
//! An unset icon is not a blank. [`default_for`] reads the command text and
//! picks the shape the command already implies, so a preset saved before this
//! existed still looks deliberate, and picking an icon is an override rather
//! than a chore.

use dioxus::prelude::*;

/// One icon: what it is called on disk, what it is called to a person, and
/// what it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icon {
    /// Stable identifier written into `launch.json`. Never renamed: the slug
    /// is the only thing a saved preset stores, so changing one silently
    /// swaps the operator's chosen icon for the default.
    pub slug: &'static str,
    /// What the picker's tooltip says.
    pub label: &'static str,
    /// Stroked subpaths: `fill: none`, `stroke: currentColor`.
    pub stroke: &'static str,
    /// Solid subpaths, or `""` when the icon has none.
    pub fill: &'static str,
}

/// Every icon this build offers, in picker order.
///
/// Ordered by how often a command wants one, not alphabetically: a grid the
/// operator scans left to right should reach `terminal` before `flask`.
///
/// Pairs differ by outline rather than by detail, for the reason
/// [`crate::agent`] gives: at 16px detail is the first thing to go, and a
/// picker whose shapes need squinting at is a picker that gets one click and
/// no second one.
pub const ALL: [Icon; 14] = [
    Icon {
        slug: "terminal",
        label: "Terminal",
        stroke: "M3.4 4.6L7 8L3.4 11.4M8.8 11.4H12.8",
        fill: "",
    },
    Icon {
        slug: "spark",
        label: "Spark",
        stroke: "M8 1.2Q9.35 6.65 14.8 8Q9.35 9.35 8 14.8Q6.65 9.35 1.2 8Q6.65 6.65 8 1.2Z",
        fill: "",
    },
    Icon {
        slug: "ring",
        label: "Ring",
        stroke: "M2.5 8a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0-11 0",
        fill: "M6.5 8a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0",
    },
    Icon {
        slug: "hexagon",
        label: "Hexagon",
        stroke: "M8 2.1L13.11 5.05L13.11 10.95L8 13.9L2.89 10.95L2.89 5.05Z",
        fill: "",
    },
    Icon {
        slug: "bars",
        label: "Bars",
        stroke: "M3.25 4.5H12.75M3.25 8H9.75M3.25 11.5H7.25",
        fill: "",
    },
    Icon {
        slug: "brackets",
        label: "Brackets",
        stroke: "M3 6V4.5A1.5 1.5 0 0 1 4.5 3H6M10 3H11.5A1.5 1.5 0 0 1 13 4.5V6\
                 M13 10V11.5A1.5 1.5 0 0 1 11.5 13H10M6 13H4.5A1.5 1.5 0 0 1 3 11.5V10",
        fill: "",
    },
    Icon {
        slug: "branch",
        label: "Branch",
        stroke: "M5 4.6V11.4M5 6.6H8.6A2.4 2.4 0 0 0 11 4.2",
        fill: "M3.6 12.4a1.4 1.4 0 1 0 2.8 0a1.4 1.4 0 1 0-2.8 0\
               M9.6 3.4a1.4 1.4 0 1 0 2.8 0a1.4 1.4 0 1 0-2.8 0",
    },
    Icon {
        slug: "wrench",
        label: "Wrench",
        stroke: "M10.4 2.6A3.4 3.4 0 0 0 6.6 7.1L3 10.7A1.6 1.6 0 0 0 5.3 13L8.9 9.4\
                 A3.4 3.4 0 0 0 13.4 5.6L11.2 7.8L9.2 7.4L8.8 5.4Z",
        fill: "",
    },
    Icon {
        slug: "flask",
        label: "Flask",
        stroke: "M6.4 2.4V6.2L3.3 11.6A1.3 1.3 0 0 0 4.4 13.6H11.6A1.3 1.3 0 0 0 12.7 11.6\
                 L9.6 6.2V2.4M5.6 2.4H10.4",
        fill: "",
    },
    Icon {
        slug: "container",
        label: "Container",
        stroke: "M2.6 5.4L8 2.6L13.4 5.4L8 8.2ZM2.6 5.4V10.6L8 13.4L13.4 10.6V5.4M8 8.2V13.4",
        fill: "",
    },
    Icon {
        slug: "search",
        label: "Search",
        stroke: "M3 7a4 4 0 1 0 8 0a4 4 0 1 0-8 0M9.9 9.9L13.2 13.2",
        fill: "",
    },
    Icon {
        slug: "pencil",
        label: "Pencil",
        stroke: "M10.6 2.8L13.2 5.4L5.6 13H3V10.4ZM9.2 4.2L11.8 6.8",
        fill: "",
    },
    Icon {
        slug: "play",
        label: "Play",
        stroke: "M5.4 3.2L12.4 8L5.4 12.8Z",
        fill: "",
    },
    Icon {
        slug: "bolt",
        label: "Bolt",
        stroke: "M9.4 1.8L4.2 8.8H7.6L6.6 14.2L11.8 7.2H8.4Z",
        fill: "",
    },
];

/// The icon a slug names, or `None` when this build has never had one.
///
/// `None` rather than a substitute, because the two callers want different
/// things from a stale slug: [`resolve`] falls back to the command's own
/// shape, and the picker wants to know that nothing is selected.
pub fn from_slug(slug: &str) -> Option<&'static Icon> {
    ALL.iter().find(|icon| icon.slug == slug)
}

/// The icon to draw for a command with no chosen icon.
///
/// Read off the program name, so the common commands arrive already
/// distinguishable and an operator who never opens the picker still gets a
/// list they can scan. Matched exactly on the basename, never as a prefix:
/// `gitk` is not `git`, and a confident wrong icon is worse than the generic
/// one.
pub fn default_for(command_line: &str) -> &'static Icon {
    let program = command_line
        .split_whitespace()
        .next()
        .unwrap_or(command_line);
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let lower = base.to_ascii_lowercase();
    let name = EXECUTABLE_SUFFIXES
        .iter()
        .find_map(|suffix| lower.strip_suffix(suffix))
        .unwrap_or(lower.as_str());

    let slug = BY_COMMAND
        .iter()
        .find(|(command, _)| *command == name)
        .map(|(_, slug)| *slug)
        .unwrap_or(FALLBACK);
    from_slug(slug).unwrap_or(&ALL[0])
}

/// The icon to draw, given what was stored.
///
/// An unknown slug is treated exactly as an unset one. A profile written by a
/// newer build, or hand-edited, must still open: dropping the whole preset
/// over a nine-character string would lose a command the operator can see no
/// other way.
pub fn resolve(slug: Option<&str>, command_line: &str) -> &'static Icon {
    slug.and_then(from_slug)
        .unwrap_or_else(|| default_for(command_line))
}

/// Suffixes `PATHEXT` resolution can leave on a Windows command.
const EXECUTABLE_SUFFIXES: [&str; 5] = [".exe", ".cmd", ".bat", ".com", ".ps1"];

/// The icon used when nothing in [`BY_COMMAND`] matches.
///
/// The corner brackets, which claim nothing about what the command does. A
/// terminal glyph here would say "this is a shell" about every unrecognised
/// program, which is the confident wrong answer.
const FALLBACK: &str = "brackets";

/// Program basenames with an obvious shape, keyed on the command.
///
/// The agent commands mirror [`crate::agent`]'s table so a preset for `claude`
/// and a tab running `claude` are not two different pictures. Everything else
/// is a command an operator actually saves: a build, a test run, a repository
/// operation, a container, an editor, a search.
const BY_COMMAND: [(&str, &str); 40] = [
    ("claude", "spark"),
    ("codex", "hexagon"),
    ("gemini", "spark"),
    ("opencode", "bars"),
    ("veyyon", "ring"),
    ("sh", "terminal"),
    ("bash", "terminal"),
    ("zsh", "terminal"),
    ("fish", "terminal"),
    ("dash", "terminal"),
    ("ksh", "terminal"),
    ("nu", "terminal"),
    ("pwsh", "terminal"),
    ("powershell", "terminal"),
    ("cmd", "terminal"),
    ("git", "branch"),
    ("jj", "branch"),
    ("gh", "branch"),
    ("cargo", "wrench"),
    ("make", "wrench"),
    ("just", "wrench"),
    ("cmake", "wrench"),
    ("npm", "wrench"),
    ("pnpm", "wrench"),
    ("yarn", "wrench"),
    ("bun", "wrench"),
    ("go", "wrench"),
    ("pytest", "flask"),
    ("jest", "flask"),
    ("vitest", "flask"),
    ("docker", "container"),
    ("podman", "container"),
    ("kubectl", "container"),
    ("rg", "search"),
    ("grep", "search"),
    ("fd", "search"),
    ("vim", "pencil"),
    ("nvim", "pencil"),
    ("hx", "pencil"),
    ("nano", "pencil"),
];

#[derive(Props, Clone, PartialEq)]
pub struct IconGlyphProps {
    pub icon: Icon,
    /// Extra class on the `svg`, so the caller sizes it for its own surface.
    #[props(default = String::new())]
    pub class: String,
}

/// Draw one icon.
///
/// `aria-hidden`, always. Every surface that draws one also writes the label
/// in text or in a `title`, so a screen reader announcing the shape as well
/// would say the same thing twice.
#[component]
pub fn IconGlyph(props: IconGlyphProps) -> Element {
    rsx! {
        svg {
            class: "rg-icon {props.class}",
            view_box: "0 0 16 16",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.25",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "{props.icon.stroke}" }
            if !props.icon.fill.is_empty() {
                path { d: "{props.icon.fill}", fill: "currentColor", stroke: "none" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct IconPickerProps {
    /// The slug currently stored, if any.
    pub selected: Option<String>,
    /// The command the entry runs, so the unset state can show what it would
    /// draw instead of an empty box.
    pub command_line: String,
    /// `None` when the operator clears the choice back to the default.
    pub on_pick: EventHandler<Option<String>>,
}

/// Choose the icon for a saved command.
///
/// A flat grid of every icon plus one Default cell, not a dropdown: the whole
/// set is fourteen shapes and a menu that hides thirteen of them to save one
/// row costs a click to learn what the choices even are.
///
/// The Default cell draws [`default_for`] rather than a blank, so the operator
/// can see what they are keeping before they override it.
#[component]
pub fn IconPicker(props: IconPickerProps) -> Element {
    let chosen = props.selected.clone();
    let implied = default_for(&props.command_line);
    let unset = chosen.as_deref().and_then(from_slug).is_none();

    rsx! {
        div { class: "rg-iconpick", role: "radiogroup", aria_label: "Icon",
            button {
                class: if unset { "rg-iconpick__cell rg-iconpick__cell--on" } else { "rg-iconpick__cell" },
                r#type: "button",
                role: "radio",
                aria_checked: unset.to_string(),
                title: "Default ({implied.label})",
                onclick: move |_| props.on_pick.call(None),
                IconGlyph { icon: *implied, class: "rg-iconpick__glyph" }
            }
            for icon in ALL.iter() {
                {
                    let on = chosen.as_deref() == Some(icon.slug);
                    let slug = icon.slug;
                    rsx! {
                        button {
                            key: "{slug}",
                            class: if on { "rg-iconpick__cell rg-iconpick__cell--on" } else { "rg-iconpick__cell" },
                            r#type: "button",
                            role: "radio",
                            aria_checked: on.to_string(),
                            title: "{icon.label}",
                            onclick: move |_| props.on_pick.call(Some(slug.to_string())),
                            IconGlyph { icon: *icon, class: "rg-iconpick__glyph" }
                        }
                    }
                }
            }
        }
    }
}

/// The icon an operator did not choose still has to be the right one.
#[cfg(test)]
mod tests;
