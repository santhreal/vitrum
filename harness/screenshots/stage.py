#!/usr/bin/env python3
"""Create the session set the published screenshots are taken of.

One table, because the README's alt text is a claim about what is on screen
and the only way to keep the two in step is to have the rows written down.
Three projects, eight agents, and between them every state the sidebar can
show: blocked on an approval, working, waiting for an answer, finished unseen,
failed, and — set from the interface afterwards, because a snooze is the
operator's and never the daemon's — parked.

The daemon names a project after the directory the first session in it runs
in, so the root session of each project is created first. Every row's
directory is unique, which is also how `bin/<agent>` resolves which transcript
to play: see `rig.sh`.

usage: stage.py            create the rows, print `id<TAB>title` for each
       stage.py --table    print the table without touching a daemon
"""

import os
import sys

sys.path.insert(
    0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "remote")
)

from sessions import connect  # noqa: E402

# (project id, cwd, command, title, role)
#
# `role` is not sent to the daemon. It is the transcript the command plays,
# resolved from the directory, and it is here so one file states what each row
# is meant to be.
ROWS = [
    (1, "/src/vitrum", "codex", "sidebar row geometry", "working-core"),
    (
        1,
        "/src/vitrum/crates/vitrum-core",
        "claude",
        "scrollback trim on resize",
        "approval",
    ),
    (
        1,
        "/src/worktrees/hint-parser",
        "claude",
        "hint parser walkthrough",
        "done-hint",
    ),
    (2, "/src/veyyon", "gemini", "light theme contrast", "input"),
    (2, "/src/veyyon/crates/tui", "codex", "transcript reflow", "working-reflow"),
    (3, "/src/keyhog", "claude", "release notes", "done-notes"),
    (3, "/src/keyhog/fuzz", "opencode", "keymap chord", "failed"),
    (3, "/src/keyhog/docs", "codex", "docs chapter merge", "snoozed"),
]


def table():
    for _, cwd, _, _, role in ROWS:
        print(f"{cwd}\t{role}")


def create():
    ws = connect()
    for project, cwd, command, title, _ in ROWS:
        ws.send_json(
            {
                "t": "createSession",
                "projectId": project,
                "cwd": cwd,
                "command": command,
                "args": [],
                "cols": 120,
                "rows": 40,
                "title": title,
            }
        )
        created = ws.wait_for("sessionCreated")
        print(f"{created['id']}\t{title}", flush=True)
    ws.close()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--table":
        table()
    else:
        create()
