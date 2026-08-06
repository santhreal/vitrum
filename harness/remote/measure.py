#!/usr/bin/env python3
"""Memory and CPU for a process tree, read straight out of /proc.

This runs on the measurement host, never on a developer desktop. It takes the
same two numbers GOAL.md's table is built from:

  pss   The sum of `Pss` across `/proc/<pid>/smaps_rollup` for a root process
        and every descendant. Pss divides a shared page by the number of
        processes mapping it, so twenty windows sharing one WebKitWebProcess
        are counted once between them rather than twenty times. That is the
        only reason the 398 MB figure in GOAL.md means anything: RSS would
        count the shared engine twenty times over.

  cpu   The sum of utime + stime deltas across the same tree over a
        wall-clock window, as a percentage of ONE core. A twenty-window client
        that wakes on nothing should read hundredths of a percent.

  footprint
        The same sum as `pss` for a tree, but labelled, and falling back to
        RSS for the whole tree when any process in it will not give up
        `smaps_rollup`. The bench comparison needs it because it measures a
        foreign product as well as this one, and cannot assume the kernel
        answers for both.

Both walk the tree by ppid at the moment of the call. A process that starts or
exits inside a CPU window is reported by name rather than silently folded in,
because "the tree changed under the measurement" is the single most common way
a number like this comes out wrong.
"""

import os
import sys
import time

CLK_TCK = os.sysconf("SC_CLK_TCK")


def read_stat(pid):
    """(ppid, comm, utime_ticks, stime_ticks) for `pid`, or None if it is gone.

    `comm` can contain spaces and parentheses, so the fields after it are found
    from the LAST ')' rather than by splitting the whole line. Splitting on
    whitespace from the left misparses every process whose name contains a
    space, and `WebKitWebProcess` is one bad rename away from being one.
    """
    try:
        with open(f"/proc/{pid}/stat", "rb") as fh:
            raw = fh.read()
    except OSError:
        return None
    close = raw.rfind(b")")
    open_paren = raw.find(b"(")
    if close < 0 or open_paren < 0 or close < open_paren:
        return None
    comm = raw[open_paren + 1 : close].decode("utf-8", "replace")
    rest = raw[close + 2 :].split()
    if len(rest) < 13:
        return None
    # rest[0] is field 3 (state), so field N is rest[N - 3].
    return (int(rest[1]), comm, int(rest[11]), int(rest[12]))


def all_procs():
    """pid -> (ppid, comm, utime, stime) for every process we can read."""
    out = {}
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        st = read_stat(int(name))
        if st is not None:
            out[int(name)] = st
    return out


def tree(root, table):
    """`root` and every descendant of it, in discovery order."""
    children = {}
    for pid, (ppid, _, _, _) in table.items():
        children.setdefault(ppid, []).append(pid)
    found = []
    queue = [root]
    seen = {root}
    while queue:
        pid = queue.pop(0)
        if pid not in table:
            continue
        found.append(pid)
        for kid in sorted(children.get(pid, [])):
            if kid not in seen:
                seen.add(kid)
                queue.append(kid)
    return found


def pss_kb(pid):
    """Pss for `pid` in kB, or None when the kernel will not tell us.

    A zombie has no mappings and a process that exits mid-walk has no file at
    all. Both return None so the caller can say how many it lost instead of
    quietly reporting a smaller total.
    """
    try:
        with open(f"/proc/{pid}/smaps_rollup", "rb") as fh:
            for line in fh:
                if line.startswith(b"Pss:"):
                    return int(line.split()[1])
    except OSError:
        return None
    return None


def rss_kb(pid):
    """VmRSS for `pid` in kB, or None if it is gone.

    The fallback for a host or a process where `smaps_rollup` cannot be read.
    It is a worse number than Pss and must never be mixed with one in a total:
    RSS charges a shared page to every process mapping it, so a tree sharing
    one renderer reads far higher than it costs. `cmd_footprint` therefore
    picks one metric for the whole tree and labels it.
    """
    try:
        with open(f"/proc/{pid}/status", "rb") as fh:
            for line in fh:
                if line.startswith(b"VmRSS:"):
                    return int(line.split()[1])
    except OSError:
        return None
    return None



def mb(kb):
    return kb / 1024.0


def loadavg():
    """The 1, 5 and 15 minute load averages as a string."""
    with open("/proc/loadavg", encoding="ascii") as fh:
        return " ".join(fh.read().split()[:3])

def machine():
    """One line of context that has to travel with every number.

    A measurement that moved because the box was busy and one that moved
    because the code changed look identical in a bare figure, and the habit of
    attributing the first to the second is how a regression gets waved
    through. The cost of ruling it out is one line, so the line is always
    printed rather than offered behind a flag.
    """
    return f"cores {os.cpu_count()}, load {loadavg()}"


# Process names that belong to a vitrum client tree. `comm` in /proc/<pid>/stat
# is truncated to 15 characters, so "WebKitWebProcess" arrives as
# "WebKitWebProces" and these have to be prefixes, not equalities.
KIN_PREFIXES = ("WebKit", "vitrum")


def strays(pids, table):
    """Processes that look like ours but are NOT in the tree we measured.

    This exists because the tree walk has a real blind spot and the number it
    produces is the headline one. A descendant that gets reparented, by double
    forking or by an intermediary that exits, has its ppid set to 1 and leaves
    the tree silently. Demonstrated rather than assumed: a `setsid` grandchild
    whose parent has exited stays alive and stops being counted, and nothing in
    the output says so.

    If that ever happened to a `WebKitWebProcess` the total would lose its
    single largest contributor, around 270 MB across twenty windows, and report
    a smaller, better-looking, wrong figure. So anything wearing a family name
    and sitting outside the tree is named loudly rather than omitted quietly.
    It is a warning and not an error: on a shared box a stray may legitimately
    belong to somebody else, and only a human can say which.
    """
    out = []
    for pid, entry in table.items():
        if pid in pids:
            continue
        if entry[1].startswith(KIN_PREFIXES):
            out.append((pid, entry[1]))
    return sorted(out)


def cmd_pss(root):
    table = all_procs()
    pids = tree(root, table)
    if not pids:
        sys.exit(f"measure: no process {root}")
    total = 0
    missing = []
    rows = []
    for pid in pids:
        kb = pss_kb(pid)
        if kb is None:
            missing.append((pid, table[pid][1]))
            continue
        total += kb
        rows.append((pid, table[pid][1], kb))
    rows.sort(key=lambda r: -r[2])
    for pid, comm, kb in rows:
        print(f"  {pid:>8}  {comm:<24} {mb(kb):>9.1f} MB")
    for pid, comm in missing:
        print(f"  {pid:>8}  {comm:<24}   no smaps_rollup (exited or zombie)")
    kinds = {}
    for _, comm, kb in rows:
        acc = kinds.setdefault(comm, [0, 0])
        acc[0] += 1
        acc[1] += kb
    print()
    for comm in sorted(kinds):
        count, kb = kinds[comm]
        print(f"  {comm:<24} x{count:<3} {mb(kb):>9.1f} MB")
    print()
    outside = strays(set(pids), table)
    for pid, comm in outside:
        print(f"  WARNING {pid} {comm} looks like ours and is NOT in this tree")
    if outside:
        print(f"  {len(outside)} process(es) outside the tree; the total below excludes them")
    print(f"processes {len(rows)}")
    print(f"machine {machine()}")
    print(f"pss {mb(total):.1f} MB")


def cmd_cpu(root, seconds):
    before = all_procs()
    pids0 = set(tree(root, before))
    if not pids0:
        sys.exit(f"measure: no process {root}")
    pss0 = sum(kb for kb in (pss_kb(p) for p in pids0) if kb is not None)
    load0 = loadavg()
    t0 = time.monotonic()

    time.sleep(seconds)

    after = all_procs()
    pids1 = set(tree(root, after))
    elapsed = time.monotonic() - t0
    pss1 = sum(kb for kb in (pss_kb(p) for p in pids1) if kb is not None)

    ticks = 0
    gone = []
    fresh = []
    for pid in pids0 | pids1:
        b = before.get(pid)
        a = after.get(pid)
        if b is not None and a is not None:
            ticks += (a[2] + a[3]) - (b[2] + b[3])
        elif a is not None:
            # Started inside the window: all of its CPU belongs to the window.
            ticks += a[2] + a[3]
            fresh.append((pid, a[1]))
        else:
            # Exited inside the window. Whatever it burned before it died is
            # unrecoverable, so it is named rather than guessed at.
            gone.append((pid, b[1]))

    percent = 100.0 * (ticks / CLK_TCK) / elapsed
    load1 = loadavg()
    print(f"window {elapsed:.1f} s at {CLK_TCK} Hz")
    print(f"machine cores {os.cpu_count()}, load {load0} at the start, {load1} at the end")
    print(f"processes {len(pids0)} at the start, {len(pids1)} at the end")
    for pid, comm in fresh:
        print(f"  started during the window: {pid} {comm}")
    for pid, comm in gone:
        print(f"  exited during the window:  {pid} {comm} (its CPU is not counted)")
    print(f"cpu {percent:.4f} % of one core   ({ticks} ticks)")
    print(f"pss {mb(pss0):.1f} MB -> {mb(pss1):.1f} MB   (drift {mb(pss1 - pss0):+.1f} MB)")


def cmd_footprint(root, label):
    """One labelled memory total for a whole process tree, PSS or RSS.

    `pss` exists for the vitrum client, whose tree the harness knows. This
    exists for the comparison in `rig.sh bench`, which measures a foreign
    product too and cannot assume its kernel will hand over `smaps_rollup` for
    every process in it. So the metric is decided by what the tree actually
    yielded and printed next to the number, rather than assumed.

    One metric for the whole tree, never a mixture. A total of Pss for the
    processes that allowed it and RSS for the rest is not a quantity, and the
    comparison it feeds is the entire point of the run.
    """
    table = all_procs()
    pids = tree(root, table)
    if not pids:
        sys.exit(f"measure: no process {root}")

    seen = []
    missing = []
    for pid in pids:
        pss = pss_kb(pid)
        rss = rss_kb(pid)
        if rss is None and pss is None:
            missing.append((pid, table[pid][1]))
            continue
        seen.append((pid, table[pid][1], pss, rss))

    metric = "pss" if seen and all(pss is not None for _, _, pss, _ in seen) else "rss"
    rows = []
    for pid, comm, pss, rss in seen:
        kb = pss if metric == "pss" else rss
        if kb is None:
            missing.append((pid, comm))
            continue
        rows.append((pid, comm, kb))
    total = sum(kb for _, _, kb in rows)

    rows.sort(key=lambda r: -r[2])
    for pid, comm, kb in rows:
        print(f"  {pid:>8}  {comm:<24} {mb(kb):>9.1f} MB")
    for pid, comm in missing:
        print(f"  {pid:>8}  {comm:<24}   unreadable (exited or zombie)")

    kinds = {}
    for _, comm, kb in rows:
        acc = kinds.setdefault(comm, [0, 0])
        acc[0] += 1
        acc[1] += kb
    print()
    for comm in sorted(kinds):
        count, kb = kinds[comm]
        print(f"  {comm:<24} x{count:<3} {mb(kb):>9.1f} MB")
    print()
    if metric == "rss":
        print("  metric is RSS: at least one process would not give up smaps_rollup,")
        print("  and a shared page is charged to every process mapping it, so this")
        print("  total is an upper bound rather than the cost of the tree")
    for pid, comm in strays(set(pids), table):
        print(f"  WARNING {pid} {comm} looks like ours and is NOT in this tree")
    print(f"machine {machine()}")
    # Parsed by rig.sh to build the comparison, so the shape is fixed.
    print(f"footprint {label} metric={metric} processes={len(rows)} total_kb={total}")
    print(f"{metric} {mb(total):.1f} MB across {len(rows)} process(es)")


def main(argv):
    if len(argv) >= 3 and argv[1] == "pss":
        cmd_pss(int(argv[2]))
    elif len(argv) >= 4 and argv[1] == "cpu":
        cmd_cpu(int(argv[2]), float(argv[3]))
    elif len(argv) >= 4 and argv[1] == "footprint":
        cmd_footprint(int(argv[2]), argv[3])
    else:
        sys.exit(
            "usage: measure.py pss <pid> | measure.py cpu <pid> <seconds>"
            " | measure.py footprint <pid> <label>"
        )


if __name__ == "__main__":
    main(sys.argv)
