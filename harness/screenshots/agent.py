#!/usr/bin/env python3
"""A staged coding-agent TUI, for running as a session's command.

The published screenshots have to show agents in the states only vitrum
surfaces: one blocked on an approval, several working, one waiting for an
answer, ones that finished while the operator was looking elsewhere, one that
failed. Those states come from what the program in the PTY does — the OSC 7373
hint it declares, whether it is blocked on the terminal, and what it exits
with — so the only way to photograph them is to run a program that does those
things.

No agent vendor's binary is installed on the capture host and none of them
would produce the same bytes twice if it were, so this plays a fixed
transcript instead: the same lines, the same hint, the same ending, every run.
What is staged is the CONTENT. The session record, the hint parse, the row
state, the pane and the capture are all the real ones.

usage: agent.py --role <role> [--pause SECONDS]

Roles are the row states, one transcript each. `--list` prints them.

Nothing here prints a prompt, a directory listing, a build, a test run or a
version-control command: see AGENTS.md, "Demos show agents, not shell output".
"""

import argparse
import os
import sys
import time

# Only the OSC vitrum defines. Deliberately no OSC 0 or 2: a terminal title
# would override the session's own name in the sidebar, and the row title is
# the thing under review.
HINT = "\x1b]7373;{state}\x1b\\"
HINT_LABELLED = "\x1b]7373;{state};{label}\x1b\\"

DIM = "\x1b[38;5;245m"
BOLD = "\x1b[1m"
OFF = "\x1b[0m"
BLUE = "\x1b[38;5;110m"
GREEN = "\x1b[38;5;114m"
YELLOW = "\x1b[38;5;179m"
RED = "\x1b[38;5;174m"
MAUVE = "\x1b[38;5;140m"


def header(agent, model):
    return [
        f"{BLUE}{BOLD}{agent}{OFF}  {DIM}{model}{OFF}",
        "",
    ]


def turn(text):
    return [f"{DIM}>{OFF} {text}", ""]


def plan(steps):
    out = [f"{DIM}plan{OFF}"]
    out += [f"{GREEN}  v{OFF} {step}" for step in steps]
    out.append("")
    return out


def tool(call, detail):
    return [f"{MAUVE}  *{OFF} {call}  {DIM}{detail}{OFF}", ""]


def box(colour, title, lines):
    width = max(len(title), max((len(line) for line in lines), default=0)) + 4
    out = [f"{colour}+{'-' * width}+{OFF}"]
    out.append(f"{colour}|{OFF}  {BOLD}{title}{OFF}{' ' * (width - len(title) - 2)}{colour}|{OFF}")
    out.append(f"{colour}|{OFF}{' ' * width}{colour}|{OFF}")
    for line in lines:
        out.append(f"{colour}|{OFF}  {line}{' ' * (width - len(line) - 2)}{colour}|{OFF}")
    out.append(f"{colour}+{'-' * width}+{OFF}")
    return out


# Every role: the lines it prints, the hint it declares at the end, and how it
# ends. `hold` blocks on the terminal, which is what an agent waiting for an
# answer does and what the daemon's foreground probe reads. `exit` leaves a
# status behind for the row to settle or fail on.
ROLES = {
    "approval": {
        "lines": (
            header("Claude Code", "sonnet-4.5")
            + turn("Trim scrollback on resize instead of on the next write.")
            + plan(
                [
                    "Locate the trim call",
                    "Move it into the resize path",
                    "Keep the byte budget",
                ]
            )
            + tool("read crates/vitrum-core/src/session.rs", "resize handling")
            + [
                "Trim runs from the write path only, so a shrink leaves the old",
                "budget in place until the next line arrives.",
                "",
                f"{DIM}crates/vitrum-core/src/session.rs{OFF}",
                "  pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {",
                "      self.pty.resize(cols, rows)?;",
                f"{GREEN}+     self.scrollback.trim_to(self.budget);{OFF}",
                "  }",
                "",
            ]
            + box(
                YELLOW,
                "Approval required",
                [
                    "Write the change above to disk.",
                    "Two files, eleven lines.",
                    "",
                    "> Yes, apply it",
                    "  No, keep reading",
                    "  Show the full diff",
                ],
            )
        ),
        "hint": ("approval", "write the resize trim to disk"),
        "end": "hold",
    },
    "working-core": {
        "lines": (
            header("Codex", "gpt-5-codex")
            + turn("Give a sidebar row a stable height at every text scale.")
            + plan(["Measure the row at three scales", "Pin the tallest"])
            + tool("read app/src/ui/sidebar.rs", "row geometry")
            + ["Measuring the row box at 0.9, 1.0 and 1.25.", ""]
        ),
        "hint": ("working", "measuring row geometry"),
        "end": "hold-quiet",
    },
    "working-reflow": {
        "lines": (
            header("Codex", "gpt-5-codex")
            + turn("Reflow the transcript when the pane narrows.")
            + plan(["Find the wrap point", "Reflow from the last hard break"])
            + tool("read crates/tui/src/reflow.rs", "wrap points")
            + ["Rewrapping 4 812 lines against the new width.", ""]
        ),
        "hint": ("working", "reflowing the transcript"),
        "end": "hold-quiet",
    },
    "input": {
        "lines": (
            header("Gemini CLI", "gemini-2.5-pro")
            + turn("Raise the light theme's contrast to AA everywhere.")
            + [
                "Two token pairs land under 4.5:1 against the light surface,",
                "and both are used for secondary text.",
                "",
            ]
            + box(
                BLUE,
                "Waiting for you",
                [
                    "Which floor should the muted pair meet?",
                    "",
                    "> 4.5:1, the AA text floor",
                    "  7:1, the AAA text floor",
                ],
            )
        ),
        "hint": ("input", "which contrast floor"),
        "end": "hold",
    },
    "done-hint": {
        "lines": (
            header("Claude Code", "sonnet-4.5")
            + turn("Walk me through how the hint parser keeps a split sequence.")
            + [
                "The parser holds the bytes of a sequence in flight and nothing",
                "else, so a hint split across two reads is reassembled and a",
                "stream that opens an OSC and never closes it is abandoned at",
                "the payload cap rather than buffered.",
                "",
                f"{GREEN}Answered.{OFF} Nothing was written.",
                "",
            ]
        ),
        "hint": ("ready", "walkthrough finished"),
        "end": "exit:0",
    },
    "done-notes": {
        "lines": (
            header("Claude Code", "sonnet-4.5")
            + turn("Draft the release notes for the chord editor.")
            + [
                "Wrote 34 lines under Unreleased: the chord editor, the",
                "collision refusal, and the two bindings that moved.",
                "",
                f"{GREEN}Done.{OFF} One file changed.",
                "",
            ]
        ),
        "hint": ("ready", "notes drafted"),
        "end": "exit:0",
    },
    "failed": {
        "lines": (
            header("opencode", "qwen3-coder")
            + turn("Fuzz the chord parser for a day and triage what falls out.")
            + [
                f"{RED}The model endpoint refused the connection after 3 retries.{OFF}",
                "No corpus was written and nothing was triaged.",
                "",
            ]
        ),
        "hint": None,
        "end": "exit:1",
    },
    "snoozed": {
        "lines": (
            header("Codex", "gpt-5-codex")
            + turn("Fold the keymap chapter into the configuration chapter.")
            + [
                "Both chapters describe the same file from opposite ends. The",
                "merged chapter is 140 lines and loses nothing.",
                "",
                f"{GREEN}Ready for review.{OFF}",
                "",
            ]
        ),
        "hint": ("ready", "merged chapter ready"),
        "end": "hold-quiet",
    },
}


def play(role, pause):
    spec = ROLES[role]
    out = sys.stdout
    for line in spec["lines"]:
        out.write(line + "\r\n")
        out.flush()
        if pause:
            time.sleep(pause)
    if spec["hint"]:
        state, label = spec["hint"]
        out.write(HINT_LABELLED.format(state=state, label=label) if label else HINT.format(state=state))
        out.flush()

    end = spec["end"]
    if end.startswith("exit:"):
        return int(end.split(":", 1)[1])
    if end == "hold":
        # Blocked on the terminal, which is what an agent waiting for an answer
        # is, and what the daemon's foreground probe reads.
        try:
            sys.stdin.read(1)
        except (KeyboardInterrupt, OSError):
            pass
        return 0
    # Awake but not asking: a working row must not read as waiting.
    while True:
        time.sleep(3600)


def main(argv):
    parser = argparse.ArgumentParser(add_help=True)
    parser.add_argument("--role")
    parser.add_argument("--pause", type=float, default=0.0)
    parser.add_argument("--list", action="store_true")
    opts = parser.parse_args(argv[1:])
    if opts.list:
        for name in ROLES:
            print(name)
        return 0
    if opts.role not in ROLES:
        print(f"agent.py: unknown role {opts.role!r}", file=sys.stderr)
        return 2
    # The same terminal name the product gives a real session, so a role
    # played outside a vitrum pane writes the escapes it would write inside
    # one. Inside a pane this is already set and the default never applies.
    os.environ.setdefault("TERM", "vte-256color")
    return play(opts.role, opts.pause)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
