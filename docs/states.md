# Session states

A row carries one of five states.

| State | Source |
|---|---|
| working | observed |
| ready | observed |
| failed | observed |
| waiting for approval | declared by the agent |
| waiting for input | declared by the agent |

vitrum derives working, ready and failed from the process: what the foreground
program is blocked in, whether it is still printing, and how it exited. No
cooperation from the agent is needed.

Approval and input are not observable. An agent asking to force-push and a
shell sitting at a prompt are blocked in the same `read`. Until an agent
declares one of them, the row shows the observed state.

Some agents declare their state in the terminal title, and vitrum reads it.
Codex sets `[ ! ] Action Required`, which resolves to approval. A title is
recorded separately from the session name, so renaming a session does not
disable it.

## OSC 7373

```text
ESC ] 7373 ; <state> [ ; <label> ] ESC \
```

`<state>` is `approval`, `input`, `working` or `ready`. `<label>` is optional
text shown beside the row. Terminals that do not know the sequence ignore it.

`vitrum hint` writes it:

```sh
vitrum hint approval 'run `rm -rf build/`?'
vitrum hint input 'which file? a, b or c'
vitrum hint ready 'tests pass'
vitrum hint --clear
```

`--clear` declares `working`, the one state vitrum retires by itself once the
session goes quiet, which hands the row back to observation.

`vitrum hint` writes to stdout whether or not stdout is a terminal. It exits 0
after writing the sequence and 2 on an unknown state.

## From a shell prompt

`PROMPT_COMMAND` runs after each command, which is when the shell has gone back
to waiting:

```sh
PROMPT_COMMAND='vitrum hint ready "$(basename "$PWD")"'
```

zsh:

```sh
precmd() { vitrum hint ready "${PWD:t}" }
```

## From a wrapper

The trap matters. An agent killed mid-run must not leave a stale `working`
badge on the row.

```sh
#!/bin/sh
# ~/.local/bin/claude-vitrum
trap 'vitrum hint --clear' EXIT INT TERM
vitrum hint working "$*"
claude "$@"
vitrum hint ready 'done'
```

## From Claude Code

A Claude Code hook cannot write the sequence directly: its stdout belongs to
Claude Code and it runs with no controlling terminal, so the sequence has to
reach the PTY another way.

[`integrations/claude-code`](../integrations/claude-code) contains the hook,
the `settings.json` entries that call it, and the event mapping.
