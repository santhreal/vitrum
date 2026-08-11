"""Resident cost and processor cost of a running process tree.

Proportional set size, not resident set size. A window that shares an engine
with three other windows would otherwise be charged for all of it four times,
and the totals would not add up across the tree. PSS charges a shared page to
each sharer as a fraction, so the numbers here sum.

Usage:
  footprint.py memory --tree PID
  footprint.py cpu    --tree PID --seconds 20
  footprint.py memory --match vitrum
"""

import argparse
import json
import os
import time

CLOCK_TICK = os.sysconf("SC_CLK_TCK")


def children_of():
    """Parent pid for every live process, as a map from parent to children."""
    tree = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/stat") as fh:
                fields = fh.read().rsplit(")", 1)[1].split()
        except (OSError, IndexError):
            continue
        tree.setdefault(int(fields[1]), []).append(int(entry))
    return tree


def descendants(root):
    """`root` and everything under it.

    A tree and not a name match, because the processes that cost the most are
    not named after the product that started them: a pty child carries the
    agent's own command name, and a foreign client's helpers carry its
    toolkit's, so a match on the product's name misses exactly the ones worth
    counting.
    """
    tree = children_of()
    out, stack = [], [root]
    while stack:
        pid = stack.pop()
        out.append(pid)
        stack.extend(tree.get(pid, []))
    return out


def described(pids):
    """`(pid, comm, cmdline)` for each pid still alive."""
    rows = []
    for pid in pids:
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as fh:
                cmdline = fh.read().replace(b"\0", b" ").decode(errors="replace")
            with open(f"/proc/{pid}/comm") as fh:
                comm = fh.read().strip()
        except OSError:
            continue
        rows.append((pid, comm, cmdline.strip()))
    return rows


def selected(args):
    """The processes this run is about: a tree, or a command-line match."""
    if args.tree:
        return described(descendants(args.tree))
    return processes(args.match)


def processes(match):
    """Every live pid whose command line mentions `match`, with its name."""
    found = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as fh:
                cmdline = fh.read().replace(b"\0", b" ").decode(errors="replace")
            with open(f"/proc/{pid}/comm") as fh:
                comm = fh.read().strip()
        except OSError:
            continue
        if match in cmdline:
            found.append((pid, comm, cmdline.strip()))
    return found


def pss_bytes(pid):
    """This process's proportional set size, or `None` if it is gone."""
    try:
        with open(f"/proc/{pid}/smaps_rollup") as fh:
            for line in fh:
                if line.startswith("Pss:"):
                    return int(line.split()[1]) * 1024
    except OSError:
        return None
    return None


def cpu_ticks(pid):
    """User plus system ticks this process has spent."""
    try:
        with open(f"/proc/{pid}/stat") as fh:
            fields = fh.read().rsplit(")", 1)[1].split()
    except (OSError, IndexError):
        return None
    # After the command field, `state` is index 0, so utime and stime are 11
    # and 12. Counting from the closing parenthesis rather than by splitting
    # the whole line is what survives a process whose name contains a space.
    return int(fields[11]) + int(fields[12])


def cmd_memory(args):
    rows = []
    total = 0
    for pid, comm, cmdline in selected(args):
        pss = pss_bytes(pid)
        if pss is None:
            continue
        total += pss
        rows.append({"pid": pid, "comm": comm, "pss": pss, "cmdline": cmdline[:120]})
    rows.sort(key=lambda r: -r["pss"])
    print(
        json.dumps(
            {
                "match": args.match,
                "tree": args.tree,
                "processes": len(rows),
                "pss_total": total,
                "rows": rows,
            },
            indent=2,
        )
    )


def cmd_cpu(args):
    before = {pid: cpu_ticks(pid) for pid, _, _ in selected(args)}
    started = time.monotonic()
    time.sleep(args.seconds)
    elapsed = time.monotonic() - started
    rows = []
    total = 0
    for pid, comm, _ in selected(args):
        was = before.get(pid)
        now_ticks = cpu_ticks(pid)
        if was is None or now_ticks is None:
            continue
        seconds = (now_ticks - was) / CLOCK_TICK
        total += seconds
        rows.append({"pid": pid, "comm": comm, "cpu_seconds": seconds})
    rows.sort(key=lambda r: -r["cpu_seconds"])
    print(
        json.dumps(
            {
                "match": args.match,
                "tree": args.tree,
                "seconds": elapsed,
                "cpu_seconds": total,
                "cores": total / elapsed if elapsed else 0,
                "rows": rows,
            },
            indent=2,
        )
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)
    for name in ("memory", "cpu"):
        p = sub.add_parser(name)
        p.add_argument("--match", default="")
        p.add_argument("--tree", type=int, help="root pid; counts the whole tree")
        if name == "cpu":
            p.add_argument("--seconds", type=float, default=20.0)
    args = parser.parse_args()
    {"memory": cmd_memory, "cpu": cmd_cpu}[args.cmd](args)


if __name__ == "__main__":
    main()
