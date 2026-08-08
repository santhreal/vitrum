//! The shipped harness integrations agree with the protocol they declare into.
//!
//! `integrations/` is code this repository asks other people to install, and
//! nothing builds it: the hook is Python, its configuration is JSON, and both
//! are copied by hand into somebody else's tool. A rename in `HintState` would
//! leave them wrong on disk with every Rust test still green, and the operator
//! would find out as a badge that never appears.
//!
//! So the protocol is the source and the integration is checked against it:
//! every state a shipped configuration emits must parse, the hook must accept
//! exactly the states that exist, and every state must be accounted for in
//! writing. That last one is the point. [`HintState::ALL`] is walked rather
//! than listed here, so adding a variant turns this red until the integration
//! either wires it up or says why it does not.
//!
//! That claim was measured rather than assumed, by adding a fifth variant and
//! following what broke. The compiler goes first and it is thorough: the array
//! length in `HintState::ALL`, then the exhaustive match in `hint.rs::token`,
//! then the one in `vitrum-model`'s status resolution. None of those look at
//! `integrations/`. With every one of them satisfied and the build green, the
//! two tests below are the only things left that fail, and they name the state
//! and the file. The compiler covers the Rust; this covers what ships.

use vitrum_proto::HintState;

const HOOK: &str = include_str!("../../../integrations/claude-code/vitrum-claude-hook");
const SETTINGS: &str = include_str!("../../../integrations/claude-code/settings.json");
const GUIDE: &str = include_str!("../../../integrations/claude-code/README.md");

/// The states the shipped Claude Code configuration actually emits.
fn wired_states() -> Vec<String> {
    SETTINGS
        .split("vitrum-claude-hook ")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(|state| state.trim().to_string())
        .collect()
}

/// A configuration that emits a state the daemon will not parse is a badge that
/// never appears, and nothing at runtime reports it: an unknown token is
/// ignored on purpose, so the failure is silence.
#[test]
fn every_state_the_shipped_configuration_emits_is_one_the_protocol_knows() {
    let wired = wired_states();
    for state in &wired {
        assert!(
            HintState::parse(state).is_some(),
            "integrations/claude-code/settings.json emits {state:?}, which \
             HintState::parse rejects; the hint would be dropped in silence"
        );
    }
    assert!(
        wired.len() >= 3,
        "only {} hook commands were read from settings.json",
        wired.len()
    );
}

/// The hook's own list of accepted states is exactly the protocol's list.
///
/// The hook refuses an argument it does not recognise, which is right, but it
/// keeps that list in a Python tuple no compiler checks. Narrower than the
/// protocol and a valid state is refused; wider and it accepts one the daemon
/// will drop.
#[test]
fn the_hook_accepts_exactly_the_states_that_exist() {
    let listed = HOOK
        .split("STATES = (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the hook declares a STATES tuple");

    let mut found: Vec<String> = listed
        .split(',')
        .map(|token| token.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|token| !token.is_empty())
        .collect();
    found.sort();

    let mut expected: Vec<String> = HintState::ALL
        .iter()
        .map(|state| crate::hint::token(*state).to_string())
        .collect();
    expected.sort();

    assert_eq!(
        found, expected,
        "the hook accepts {found:?}, and the protocol defines {expected:?}"
    );
}

/// Every state is accounted for in writing, wired or not.
///
/// Three of the four are wired; `input` is deliberately not, because Claude
/// Code raises one event for two situations and guessing between them would put
/// the wrong badge on a row. That is a decision, and a decision has to be
/// written down where the person installing this can read it.
///
/// The value of this test is the state that does not exist yet. A fifth
/// variant added to `HintState` arrives here unmentioned, and this goes red
/// until somebody has decided what the integration does about it.
#[test]
fn every_state_is_either_wired_or_explained() {
    let wired = wired_states();
    for state in HintState::ALL {
        let token = crate::hint::token(state);
        if wired.iter().any(|w| w == token) {
            continue;
        }
        assert!(
            GUIDE.contains(token),
            "the protocol has a {token:?} state that this integration does not \
             emit and does not mention. Wire it up, or write down why not."
        );
    }
}
