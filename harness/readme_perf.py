#!/usr/bin/env python3
"""Turn measurement runs into the performance tables in docs/performance.md.

Numbers in prose rot silently: a figure describes a build from three months ago
and nothing says so. So the tables are not written by hand:

    harness/run.sh memory 1                     # measure, on a real host
    harness/run.sh memory 20
    harness/run.sh idle-cpu 60 20
    make perf-tables                            # snapshot, then inject
    make perf-tables-check                      # what CI runs

`snapshot` reads the free-form `report.txt` that each run leaves in
`harness/out/<run>/` and writes one JSON file with the numbers and the host
they came from. `render` turns that JSON into the regions marked
`<!-- BENCH:<name>:start -->` in docs/performance.md, either writing them
(`--inject`) or failing if they are stale (`--check`).

The check mode is the point. A hand-edited table, a snapshot nobody
regenerated, and a number invented in prose all fail the same way, in CI,
naming the region that drifted.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SNAPSHOT = ROOT / "harness" / "reports" / "readme-perf.json"
DOC = ROOT / "docs" / "performance.md"
SCHEMA = "vitrum-footprint-v1"


# ---------------------------------------------------------------------------
# Parsing the rig's reports.
#
# `report.txt` is written for a human reading it after a run, and it stays that
# way: the rig is the tool an operator uses directly. So the parser takes only
# the lines that carry a number and a name, and every one of them is asserted
# to be present rather than defaulted, because a missing line means the run
# failed and a zero would read as a result.
# ---------------------------------------------------------------------------


class ReportError(RuntimeError):
    """A report is missing a line the tables are built from."""


def _text(run: Path) -> str:
    report = run / "report.txt"
    if not report.is_file():
        raise ReportError(f"{run} has no report.txt; the run did not finish")
    return report.read_text()


def _one(pattern: str, text: str, what: str, run: Path) -> re.Match[str]:
    # MULTILINE throughout: every pattern here anchors a whole report line, and
    # without it `^` means the start of the file and matches nothing.
    found = re.search(pattern, text, re.MULTILINE)
    if not found:
        raise ReportError(f"{run.name} does not report {what}")
    return found


def _windows(text: str, run: Path) -> tuple[int, int]:
    """The window and session count the run actually achieved.

    Read from the line the rig prints after every window is mapped, never from
    the count that was requested: a run that asked for twenty windows and got
    two is exactly the failure this file must not publish as a result.
    """
    m = _one(
        r"windows (\d+) of (\d+), all in pid \d+, against (\d+) sessions",
        text,
        "a completed window count",
        run,
    )
    opened, asked, sessions = int(m[1]), int(m[2]), int(m[3])
    if opened != asked:
        raise ReportError(
            f"{run.name} opened {opened} of {asked} windows; it did not finish"
        )
    return opened, sessions


def _tree(text: str, heading: str, run: Path) -> dict:
    """One process tree's totals, from its heading to the next section.

    The report prints the client tree, then the daemon tree, then a raw process
    tree, all in one file. The section has to be cut at whichever comes next or
    the client's table lists the daemon's twenty shells as its own, which is how
    this first read.
    """
    start = text.find(heading)
    if start < 0:
        raise ReportError(f"{run.name} has no '{heading}' section")
    block = text[start + len(heading) :]
    nxt = re.search(r"^(?:\w[\w -]* tree(?:,| of)|process tree of)", block, re.MULTILINE)
    if nxt:
        block = block[: nxt.start()]

    pss = _one(r"^pss ([\d.]+) MB", block[block.find("\nprocesses") :], "a PSS total", run)
    procs = _one(r"^processes (\d+)$", block, "a process count", run)
    # The aggregate rows, `  Name   xN   X MB`, are what the table shows: a
    # per-pid row would publish pids, which mean nothing to a reader.
    rows = [
        {"name": name, "count": int(count), "mb": float(mb)}
        for name, count, mb in re.findall(
            r"^  (\S+)\s+x(\d+)\s+([\d.]+) MB$", block, re.MULTILINE
        )
    ]
    if not rows:
        raise ReportError(f"{run.name} lists no processes under '{heading}'")
    return {
        "pss_mb": float(pss[1]),
        "processes": int(procs[1]),
        "rows": sorted(rows, key=lambda r: -r["mb"]),
    }


def parse_memory(run: Path) -> dict:
    text = _text(run)
    windows, sessions = _windows(text, run)
    return {
        "run": run.name,
        "windows": windows,
        "sessions": sessions,
        "client": _tree(text, "client tree, pss", run),
        "daemon": _tree(text, "daemon tree, pss", run),
    }


def parse_idle(run: Path) -> dict:
    text = _text(run)
    windows, sessions = _windows(text, run)
    seconds = _one(r"cpu over (\d+)s with", text, "an observation window", run)
    cpu = re.findall(r"^cpu ([\d.]+) % of one core\s+\((\d+) ticks\)$", text, re.MULTILINE)
    drift = re.findall(
        r"^pss ([\d.]+) MB -> ([\d.]+) MB\s+\(drift ([+-][\d.]+) MB\)$", text, re.MULTILINE
    )
    if len(cpu) < 2 or len(drift) < 2:
        raise ReportError(f"{run.name} does not report both trees' CPU and drift")
    trees = {}
    for name, (pct, ticks), (before, after, delta) in zip(
        ("client", "daemon"), cpu, drift, strict=True
    ):
        trees[name] = {
            "cpu_percent_of_core": float(pct),
            "ticks": int(ticks),
            "pss_before_mb": float(before),
            "pss_after_mb": float(after),
            "drift_mb": float(delta),
        }
    return {
        "run": run.name,
        "windows": windows,
        "sessions": sessions,
        "seconds": int(seconds[1]),
        **trees,
    }


def parse_probe(run: Path) -> dict:
    """The host the numbers describe. A figure without one is not a figure."""
    text = _text(run)
    cpu = _one(r"^cpu (\d+) threads, (.+)$", text, "a CPU", run)
    kernel = _one(r"^kernel (.+)$", text, "a kernel", run)
    host = {
        "cpu": cpu[2].strip(),
        "threads": int(cpu[1]),
        "kernel": kernel[1].strip(),
        "run": run.name,
    }
    webkit = re.search(r"^  present  libwebkit2gtk-\S+\s+(\S+)$", text, re.MULTILINE)
    if webkit:
        host["webkitgtk"] = webkit[1]
    glibc = re.search(r"^glibc .*?([\d.]+)$", text, re.MULTILINE)
    if glibc:
        host["glibc"] = glibc[1]
    return host


# ---------------------------------------------------------------------------
# Rendering.
# ---------------------------------------------------------------------------


def _mb(value: float) -> str:
    return f"{value:.1f} MB"


# The kernel caps `comm` at 15 bytes, so `ps` reports WebKit's processes under
# names that look like typos. The table names them as WebKit does.
COMM_TRUNCATED = {
    "WebKitWebProces": "WebKitWebProcess",
    "WebKitNetworkPr": "WebKitNetworkProcess",
}


def _process(name: str) -> str:
    return COMM_TRUNCATED.get(name, name)


def _workspace_version() -> str:
    manifest = (ROOT / "Cargo.toml").read_text()
    m = re.search(r'^version = "([^"]+)"', manifest, re.MULTILINE)
    if not m:
        raise ReportError("the workspace manifest has no version")
    return m[1]


def _commit() -> str:
    done = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    return done.stdout.strip() or "unknown"


def _host_sentence(snap: dict) -> str:
    host = snap["host"]
    parts = [f"**{host['cpu']}**", f"{host['threads']} logical cores"]
    if "webkitgtk" in host:
        parts.append(f"WebKitGTK {host['webkitgtk']}")
    parts.append(f"`{host['kernel']}`")
    return ", ".join(parts)


def render_footprint(snap: dict) -> str:
    runs = sorted(snap["memory"], key=lambda r: r["windows"])
    if not runs:
        raise ReportError("the snapshot holds no memory run")

    lines = [
        f"Measured on {_host_sentence(snap)}. Every window holds a live shell "
        f"against one `vitrum-server`. The figure is PSS, which charges shared "
        f"pages once, so the totals below add up across processes instead of "
        f"counting the same engine twice. `vitrum {snap['vitrum_version']}` at "
        f"`{snap['commit']}`.",
        "",
        "| Windows | Client tree | Client processes | Daemon tree | Daemon processes |",
        "|---:|---:|---:|---:|---:|",
    ]
    for run in runs:
        lines.append(
            f"| {run['windows']} | {_mb(run['client']['pss_mb'])} "
            f"| {run['client']['processes']} | {_mb(run['daemon']['pss_mb'])} "
            f"| {run['daemon']['processes']} |"
        )

    if len(runs) >= 2:
        first, last = runs[0], runs[-1]
        added = last["windows"] - first["windows"]
        if added > 0:
            per = (last["client"]["pss_mb"] - first["client"]["pss_mb"]) / added
            lines += [
                "",
                f"The {last['windows']}-window client tree is still "
                f"{last['client']['processes']} processes, not "
                f"{last['client']['processes'] * last['windows']}: every window "
                f"is a view onto one shared web process and one network process. "
                f"Going from {first['windows']} to {last['windows']} windows costs "
                f"**{per:.1f} MB per extra window**.",
            ]

    biggest = runs[-1]
    lines += ["", f"Where the {biggest['windows']}-window client tree goes:", ""]
    lines += ["| Process | Count | PSS |", "|---|---:|---:|"]
    for row in biggest["client"]["rows"]:
        lines.append(f"| `{_process(row['name'])}` | {row['count']} | {_mb(row['mb'])} |")

    lines += [
        "",
        f"The daemon side of the same run is {_mb(biggest['daemon']['pss_mb'])} "
        f"across {biggest['daemon']['processes']} processes, and the session "
        f"shells are most of it:",
        "",
        "| Process | Count | PSS |",
        "|---|---:|---:|",
    ]
    for row in biggest["daemon"]["rows"]:
        lines.append(f"| `{_process(row['name'])}` | {row['count']} | {_mb(row['mb'])} |")

    lines += [
        "",
        "Reproduce: `harness/run.sh memory 1` and `harness/run.sh memory 20`, "
        "then `make perf-tables`.",
    ]
    return "\n".join(lines)


def render_idle(snap: dict) -> str:
    idle = snap.get("idle")
    if not idle:
        raise ReportError("the snapshot holds no idle run")
    lines = [
        f"An idle terminal should cost nothing. Measured over "
        f"{idle['seconds']} s with {idle['windows']} windows open, every one "
        f"holding a live shell, on {_host_sentence(snap)}.",
        "",
        "| Tree | CPU | PSS before | PSS after | Drift |",
        "|---|---:|---:|---:|---:|",
    ]
    for name in ("client", "daemon"):
        tree = idle[name]
        lines.append(
            f"| {name.capitalize()} | {tree['cpu_percent_of_core']:.4f}% of one core "
            f"| {_mb(tree['pss_before_mb'])} | {_mb(tree['pss_after_mb'])} "
            f"| {tree['drift_mb']:+.1f} MB |"
        )
    lines += [
        "",
        f"That is {idle['client']['ticks']} scheduler ticks in "
        f"{idle['seconds']} seconds across {idle['windows']} windows. Nothing "
        f"polls, so nothing accumulates: the drift is the point of the last "
        f"column.",
        "",
        f"Reproduce: `harness/run.sh idle-cpu {idle['seconds']} {idle['windows']}`, "
        "then `make perf-tables`.",
    ]
    return "\n".join(lines)


RENDERERS = {"footprint": render_footprint, "idle": render_idle}


# ---------------------------------------------------------------------------
# The regions.
# ---------------------------------------------------------------------------


def region_bounds(text: str, name: str) -> tuple[int, int]:
    start = f"<!-- BENCH:{name}:start -->"
    end = f"<!-- BENCH:{name}:end -->"
    at = text.find(start)
    if at < 0:
        raise ReportError(f"{DOC.name} has no {start}")
    close = text.find(end, at)
    if close < 0:
        raise ReportError(f"{DOC.name} opens BENCH:{name} and never closes it")
    return at + len(start), close


def rendered(snap: dict) -> dict[str, str]:
    return {name: render(snap) for name, render in RENDERERS.items()}


def apply(text: str, blocks: dict[str, str]) -> str:
    # Late regions first, so replacing one does not move the next one's bounds.
    for name in sorted(blocks, key=lambda n: -region_bounds(text, n)[0]):
        open_at, close_at = region_bounds(text, name)
        text = f"{text[:open_at]}\n{blocks[name]}\n{text[close_at:]}"
    return text


def cmd_snapshot(args: argparse.Namespace) -> int:
    snap = {
        "schema": SCHEMA,
        "generated": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "vitrum_version": _workspace_version(),
        "commit": _commit(),
        "host": parse_probe(Path(args.probe)),
        "memory": [parse_memory(Path(run)) for run in args.memory],
        "idle": parse_idle(Path(args.idle)) if args.idle else None,
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(snap, indent=2, sort_keys=True) + "\n")
    print(f"wrote {out.relative_to(ROOT)} from {len(snap['memory'])} memory run(s)")
    return 0


def cmd_render(args: argparse.Namespace) -> int:
    snap = json.loads(Path(args.snapshot).read_text())
    if snap.get("schema") != SCHEMA:
        print(
            f"{args.snapshot} is schema {snap.get('schema')!r}, this tool writes "
            f"{SCHEMA!r}; regenerate it with `make perf-tables`",
            file=sys.stderr,
        )
        return 1

    doc = Path(args.doc)
    before = doc.read_text()
    after = apply(before, rendered(snap))

    if args.check:
        if before == after:
            print(f"{len(RENDERERS)} region(s) in {doc.name} match the snapshot")
            return 0
        stale = [
            name
            for name, block in rendered(snap).items()
            if apply(before, {name: block}) != before
        ]
        print(
            f"{doc.name} is out of date in: {', '.join(stale)}\n"
            f"The tables are generated from {Path(args.snapshot).name}. Run "
            f"`make perf-tables` and commit the result; do not edit a BENCH "
            f"region by hand.",
            file=sys.stderr,
        )
        return 1

    doc.write_text(after)
    print(
        f"{'updated' if before != after else 'unchanged'}: "
        f"{', '.join(sorted(RENDERERS))} in {doc.name}"
    )
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)

    snap = sub.add_parser("snapshot", help="parse harness runs into the JSON snapshot")
    snap.add_argument("--probe", required=True, help="a probe run directory")
    snap.add_argument(
        "--memory", action="append", default=[], required=True, help="a memory run"
    )
    snap.add_argument("--idle", help="an idle-cpu run")
    snap.add_argument("--out", default=str(SNAPSHOT))
    snap.set_defaults(func=cmd_snapshot)

    render = sub.add_parser("render", help="inject or check the generated regions")
    render.add_argument("--snapshot", default=str(SNAPSHOT))
    render.add_argument("--doc", default=str(DOC))
    render.add_argument(
        "--check", action="store_true", help="fail if the tables are stale"
    )
    render.set_defaults(func=cmd_render)

    args = parser.parse_args(argv[1:])
    try:
        return args.func(args)
    except ReportError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
