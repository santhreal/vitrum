use super::*;

fn preset(label: &str, command: &str, args: &[&str]) -> SavedPreset {
    SavedPreset {
        id: launch::mint_preset_id(label, command),
        label: label.to_string(),
        command: command.to_string(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        cwd: None,
        shortcut: None,
        icon: None,
    }
}

/// A shell that exists on every machine the suite runs on, so a fixture is
/// never reported as broken for being absent.
fn real_command() -> &'static str {
    if cfg!(windows) { "cmd" } else { "sh" }
}

#[derive(Props, Clone, PartialEq)]
struct HarnessProps {
    presets: Vec<SavedPreset>,
}

/// The handler has to be built INSIDE a component: `EventHandler::new` needs
/// a live Dioxus runtime and panics when a test constructs the props up front.
#[component]
fn Harness(props: HarnessProps) -> Element {
    rsx! {
        Presets {
            presets: props.presets.clone(),
            here: "/src/vitrum".to_string(),
            on_launch: move |_: Launch| {},
        }
    }
}

fn render(presets: Vec<SavedPreset>) -> String {
    let mut dom = VirtualDom::new_with_props(Harness, HarnessProps { presets });
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// The whole reason the band exists: a preset the operator saved is on screen
/// without typing anything, wearing the name they gave it rather than the
/// command line behind it.
#[test]
fn a_saved_preset_is_drawn_by_its_own_name() {
    let html = render(vec![preset("Plan mode", real_command(), &["-c", "true"])]);
    assert!(
        html.contains(">Plan mode<"),
        "the label is not drawn: {html}"
    );
}

/// Presets are drawn in the order the profile holds them. Ranking them would
/// move a button under the operator's hand between one open and the next,
/// which is the failure a fixed band of chips exists to avoid.
#[test]
fn the_chips_keep_the_order_they_were_saved_in() {
    let cmd = real_command();
    let html = render(vec![
        preset("First", cmd, &["-c", "true"]),
        preset("Second", cmd, &["-c", "true"]),
        preset("Third", cmd, &["-c", "true"]),
    ]);
    let order: Vec<usize> = ["First", "Second", "Third"]
        .iter()
        .map(|label| {
            html.find(label)
                .unwrap_or_else(|| panic!("{label} is not drawn: {html}"))
        })
        .collect();
    assert!(
        order[0] < order[1] && order[1] < order[2],
        "the chips are not in saved order: {html}"
    );
}

/// An operator who bound a chord has to be able to see it. The band is the
/// only surface that shows a preset and its key together, so a chord that
/// rendered nowhere would be a binding they had to remember unaided.
#[test]
fn a_bound_preset_shows_the_keys_that_fire_it() {
    let mut p = preset("Plan mode", real_command(), &["-c", "true"]);
    p.shortcut = Some("Ctrl+3".to_string());
    let html = render(vec![p]);
    assert!(
        html.contains("Ctrl+3"),
        "the chord is not drawn beside the chip: {html}"
    );
}

/// A preset whose command is gone must still be visible. Hiding it leaves the
/// operator wondering where their button went, with nothing to act on; the
/// chip stays and the tooltip carries the reason.
#[test]
fn a_broken_preset_stays_on_screen_and_says_what_is_wrong() {
    let html = render(vec![preset(
        "Ghost",
        "vitrum-no-such-command-9f3a",
        &["--go"],
    )]);
    assert!(html.contains(">Ghost<"), "the chip vanished: {html}");
    // Matched either side of the apostrophe rather than across it. The
    // renderer escapes it, and which entity it picks is its business, not a
    // fact this test should pin.
    assert!(
        html.contains("vitrum-no-such-command-9f3a is not on this machine")
            && html.contains("s PATH."),
        "the chip does not carry the fault: {html}"
    );
}

/// Nothing saved draws nothing at all. An empty band with a heading teaches
/// that presets exist while giving no way to make one, and it would push the
/// ranked list down the sheet on every fresh machine.
#[test]
fn nothing_saved_draws_nothing() {
    assert_eq!(render(Vec::new()).trim(), "");
}

/// A class this component emits with no rule behind it paints nothing, and
/// nothing is exactly what you cannot see in a screenshot. The launcher has
/// carried this guard since it was written; a component that draws into the
/// same sheet needs it for the same reason, and reading the source rather
/// than keeping a list is what stops the list going stale.
#[test]
fn every_class_the_band_emits_is_styled() {
    let src = include_str!("../presets.rs");
    let launcher = include_str!("../../../assets/parts/22-launcher.css");
    let markup = src
        .split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before);

    let mut seen: Vec<&str> = Vec::new();
    for (at, _) in markup.match_indices("class: \"") {
        let rest = &markup[at + 8..];
        let Some(end) = rest.find('"') else { continue };
        for token in rest[..end].split_whitespace() {
            if token.starts_with("rg-") && !token.contains('{') {
                seen.push(token);
            }
        }
    }
    seen.sort_unstable();
    seen.dedup();
    assert!(
        seen.len() >= 6,
        "only found {} classes; the extraction broke rather than the markup",
        seen.len()
    );
    for class in &seen {
        assert!(
            launcher.contains(&format!(".{class}")),
            "22-launcher.css has no rule for .{class}"
        );
    }
}
