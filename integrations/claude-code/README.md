# Claude Code

Make the sidebar show when a Claude Code session is waiting for you.

Without this, vitrum can tell that a session is producing output and that it has
gone quiet. It cannot tell the difference between an agent asking permission to
force-push and an agent thinking hard about a file, because both of those block
on the same read from the pty. So the two states worth interrupting you for,
`approval` and `input`, are the two vitrum will not guess. An agent has to
declare them, and `vitrum hint` is how.

This directory wires that up for Claude Code.

## Install

Copy the hook onto your path:

    install -m 755 integrations/claude-code/vitrum-claude-hook ~/.local/bin/

Then merge `settings.json` into `~/.claude/settings.json`, or into
`.claude/settings.json` inside a project to scope it there. If you already have
a `hooks` block, add these entries to it rather than replacing it.

The events map to states like this:

| Claude Code event                        | vitrum state | what the row shows        |
| ---------------------------------------- | ------------ | ------------------------- |
| `Notification` / `permission_prompt`     | `approval`   | amber, counted as waiting |
| `Notification` / `elicitation_dialog`    | `input`      | amber, counted as waiting |
| `UserPromptSubmit`                       | `working`    | working                   |
| `Stop`                                   | `ready`      | finished                  |

`Notification` fires for several unrelated things, so each entry matches one
`notification_type` rather than the event. Without a matcher, an
`auth_success` notification puts an `approval` badge on a row that is not
waiting for anything, and a wrong badge is worse than no badge.

The types nothing here emits a hint for are `idle_prompt`, which `Stop` has
already reported, and `auth_success`, `elicitation_complete`,
`elicitation_response`, `agent_needs_input` and `agent_completed`, none of
which say anything about the session in the pane.

## Checking it works

The hook is silent by design: a hook that writes to stdout corrupts the
transcript, and one that fails must never take the agent down with it. To watch
it, point it at a log file:

    "env": { "VITRUM_HOOK_LOG": "/tmp/vitrum-hook.log" }

Each event appends one line saying how many bytes went to which pty, or why
nothing did.

## How it reaches the terminal

`vitrum hint` writes an escape sequence to stdout, and any terminal that does
not recognise it ignores it. That works from a shell prompt or an agent wrapper,
where stdout is the session.

A hook is neither. Claude Code owns the hook's stdout and reads it as protocol,
and it runs hooks with no controlling terminal at all, so `/dev/tty` fails with
`ENXIO` instead of finding the session. The sequence still has to arrive on the
pty vitrum is reading.

So the hook walks up its own process tree, nearest ancestor first, looking for a
process holding a `/dev/pts` device open, and writes there. That ancestor is the
agent, running in the pane.

This is why the hook is Linux-only: it reads `/proc` for the process tree and
for the file descriptors. The `vitrum hint` command it calls is portable, and a
harness that already owns its stdout does not need any of this machinery.

## If nothing appears

- `vitrum` has to be on the hook's path, which is the path Claude Code was
  started with, not the one in your interactive shell.
- The session has to be running inside vitrum. The hook writes to a pty, and
  outside a vitrum pane there is nothing reading the sequence.
- Set `VITRUM_HOOK_LOG` and read the reason.
