# vitrum-model

The headless list model behind a sidebar of many concurrent agent sessions. It
orders and classifies. It never renders: no widgets, no strings for layout, no
colours, no I/O, no clock of its own.

A row is described on three independent axes, and keeping them independent is
the whole design:

| Axis | Owner | Question |
|------|-------|----------|
| Lifecycle | the operating system | Is the process alive? |
| Status | the agent | What is it doing? |
| Disposition | the operator | Am I done with it? |

Conflating status and disposition is what makes a twenty-session list unusable.
"Done" means the agent finished. "Settled" means *you* are finished.

```rust
use vitrum_model::{Clock, Disposition, DispositionPolicy, SessionView, arrange};

let clock = Clock::new(1_700_000_000_000, 0);
let rows: Vec<SessionView> = Vec::new();
let arranged = arrange(&rows, &DispositionPolicy::default(), clock);
assert!(arranged.active.is_empty());
assert_eq!(Disposition::Active.section().to_string(), "Active");
```

Nothing here schedules anything. A snooze expires because the derived answer
changes, not because a timer fired, so a parked list of snoozed sessions costs
no CPU at all.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
