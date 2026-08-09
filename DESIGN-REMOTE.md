# Design sketch: connecting from anywhere

Status: **a sketch, not a plan.** Nothing here is built. It exists to be argued
with before any of it is.

The goal: run twenty or thirty agents on a machine
somewhere, open a window onto them from a laptop, close the laptop, open it
again in another city, and find every agent exactly where it was. Connect from
the server itself and see the same thing.

---

## The one constraint that decides the rest

`vitrum-server` binds loopback and nothing else, and the reason is already in
the source:

> The daemon spawns arbitrary processes on request, so a listener reachable from
> the network would be remote code execution rather than a feature.

That is not a limitation to work around. It is the security model, and it
settles the transport question by itself: **we do not put the daemon on the
network. We tunnel to it over SSH.**

Which means vitrum authenticates nobody, encrypts nothing, and stores no
credentials. SSH already does all three, the operator already has it configured,
and every key rotation, jump host, hardware token and `~/.ssh/config` alias they
already use keeps working. This is what VS Code Remote does, for the same
reason.

**Anything that proposes binding `0.0.0.0` is rejected on arrival**, including
"just for a trusted LAN". A vitrum port on a network is a remote shell for
whoever finds it.

---

## What already works, unchanged

More of this exists than you would expect, because the daemon/window split was
built for multiple local windows and remote is the same shape:

| | |
|---|---|
| The daemon owns the PTYs | sessions already outlive every window |
| A window is a client | `--server ws://host:port` exists today |
| Many windows, one daemon | already in sync; every change fans out |
| Byte-exact resume | `resume_seq = from_seq + data.len()`, and the code says why an off-by-one corrupts a CSI escape |
| Loss-of-output is reported, not hidden | `gap_notice` tells a client what it missed and where to resume |

So the remote story is not a rewrite. Today, this already works:

```sh
ssh -N -L 7737:127.0.0.1:7737 host &
vitrum --server ws://127.0.0.1:7737
```

Everything below is about making that robust and making it one command.

---

## What has to be built

### 1. `vitrum --remote user@host`

One command that does what the two above do, and cleans up after itself.

```
vitrum --remote user@host
  ├─ ssh user@host  → is a daemon listening on 7737?
  │                    no → start one, DETACHED (see §2)
  ├─ ssh -N -L <free local port>:127.0.0.1:7737 user@host
  └─ connect the window to ws://127.0.0.1:<free local port>
```

The local port is chosen at random from the ephemeral range, not fixed, so two
remotes do not collide and a stale tunnel never captures a new session.

`--remote` takes exactly what `ssh` takes, and is passed to `ssh` verbatim. No
parsing of user, host and port into our own struct, because `~/.ssh/config`
aliases, `ProxyJump` and per-host keys must keep working, and any parser we
write is a parser that eventually disagrees with theirs.

### 2. Daemon lifetime on the remote

This is the part that matters for twenty or thirty agents, and it is the part
that is easy to get wrong.

**A daemon started as a child of the SSH session dies with it.** Close the
laptop, the connection drops, sshd reaps the session, and every PTY under it
goes with it. That is the failure the operator is specifically asking to avoid,
and a naive `ssh host vitrum-server` produces it.

The daemon must be started so that it belongs to the machine, not to the
connection:

- **Preferred: a systemd user service** plus `loginctl enable-linger $USER`.
  Lingering is what makes a user's services survive logout, and without it a
  user unit is also torn down when the last session ends. `Restart=on-failure`
  gets it back after a crash.
- **Fallback: `setsid`**, for hosts without systemd. Detaches from the
  controlling terminal so the session teardown does not reach it.

`--remote` should install the user unit on first use, say that it did, and say
plainly when lingering is not enabled, because on that host the promise is not
being kept.

### 3. Reconnect, which today does not exist

There is no automatic reconnect anywhere in the program, deliberately:

> Idle cost is a design constraint. There is no timer, no polling loop, no
> animation, and no automatic reconnect anywhere in this program.

That is right for a local daemon, which does not go away. It is wrong for a
laptop that closes its lid, and remote makes reconnect mandatory.

The constraint survives if reconnect is **event-driven with a bounded
schedule**, not a poll:

- the socket closing is an event, and it is what starts the schedule
- each retry is one one-shot timer, the same shape as the notice timer that
  already exists
- the schedule backs off, and it **stops**: connected, or exhausted and the
  window says so
- **a connected window schedules nothing.** Idle cost at rest is unchanged

On reconnect the client re-lists, then re-attaches each visible session with
the `resume_seq` it already holds. The splice machinery for this exists and is
tested; reconnect is a new caller of it, not new logic.

### 4. Which state belongs to the server

The requirement is that *every window, even the one on the
server itself, is managed by the server process.*

Today all persisted state lives on the client, in `ui.json`. Connect from a
laptop and then from the server itself and you get two different sidebars over
the same sessions: different workspaces, different folders, different filing.
That is wrong, and the fix is a split the code has **already made**:

| | | |
|---|---|---|
| `DaemonState` | workspaces, folders, session placement, settings | belongs to the **server** |
| `WindowState` | window geometry, which session this window shows, sidebar width | belongs to the **client** |

`Persisted` already separates these, with `restore_daemon` and
`restore_window`. Only the persistence is in the wrong place. So:

- the daemon owns the daemon half and serves it on connect
- the client keeps only the window half, per host
- a laptop and the server itself show the same workspaces over the same
  sessions, and each keeps its own window size

This is the largest piece of work here and the one most worth doing, because it
is what makes the promise true rather than approximately true.

---

## What still kills a PTY

Sessions survive **client** loss today, and will survive network loss once §3
lands. They do **not** survive **daemon** loss, and no amount of reconnect
changes that: the PTY children belong to the daemon process. If it is killed,
OOMs, or is upgraded, every agent dies with it.

Surviving that needs the PTYs to outlive the process that made them —
reparenting to `init`, a `dtach`/`tmux` shape, with the daemon reattaching to
sockets it did not create. That is a real change to `vitrum-core`, not a flag,
and it should be a separate decision from this one.

What can be done cheaply, and should be, in decreasing order of value:

1. **`Restart=on-failure`** in the user unit. Sessions still die, but the daemon
   comes back and says so instead of leaving a window connected to nothing.
2. **Never tie the daemon to the SSH session** (§2). This is the common cause
   and it is free to avoid.
3. **Say it out loud.** A window whose daemon restarted must not silently show
   an empty sidebar. It knows the difference between "no sessions" and "the
   daemon I was talking to is not the one I am talking to now", and it should
   say which.

Until reparenting exists, the README should claim exactly this and no more:
**your agents survive losing the client. They do not survive losing the
server.**

---

## Order of work

1. **§4, the state split.** Largest, most valuable, and independent of SSH: it
   improves multi-window on one machine today.
2. **§3, reconnect.** Needed by everything remote; useful locally when the
   daemon is restarted.
3. **§2, daemon lifetime.** Small, and the difference between the promise
   holding and not.
4. **§1, `--remote`.** Convenience over the three above. Deliberately last: the
   manual `ssh -L` two-liner works today, so this is ergonomics, not capability.

## Open questions

- **Does `--remote` manage the tunnel, or document it?** Spawning and
  supervising `ssh` is a process-management problem we would own forever.
  Documenting the two-liner costs the operator one command and costs us
  nothing. Worth deciding before building §1.
- **Version skew.** A 0.2 client against a 0.1 daemon. The protocol has a
  version in the handshake; nothing enforces it yet, and remote is where skew
  actually happens.
- **One client or many?** Two laptops on one remote daemon is the same code path
  as two local windows, which already works. Worth confirming rather than
  assuming.
- **Scrollback over a slow link.** `backfill_max_bytes` is tuned for loopback.
  Two megabytes on a hotel connection is a different question.
