"""Pair the frame arms and say whether carrying the probe costs anything.

`harness/frame.sh` runs two binaries alternately, one round each, and leaves a
report per round. This reads them, pairs round k of one arm against round k of
the other, and reports:

  off_minus_absent  what carrying the probe costs when it is off
  noise             what the same arm measured against itself is worth here
  on_minus_off      what recording costs, which is expected to be positive

The verdict is not a threshold anyone chose. `off_minus_absent` is zero cost
when its bootstrap interval overlaps the noise band, because a difference
smaller than the machine's own round-to-round variation is a difference this
method cannot see, and claiming otherwise would be reading noise.

Usage:
  frame_compare.py <run-dir>            one JSON document on stdout
  frame_compare.py --text <run-dir>     the same numbers as a table
"""

import glob
import json
import os
import random
import statistics
import sys

# Fixed so a verdict is reproducible from the same reports. The bootstrap is a
# confidence interval, not a source of entropy.
BOOTSTRAP_SEED = 20260811
BOOTSTRAP_RESAMPLES = 10000


def load_rounds(run_dir, build):
    """Per-round arm results for one build, in round order."""
    rounds = []
    pattern = os.path.join(run_dir, build, "*", "*", "report.json")
    for path in sorted(glob.glob(pattern), key=lambda p: int(p.split(os.sep)[-3])):
        with open(path, encoding="utf-8") as fh:
            report = json.load(fh)
        rounds.append(report)
    return rounds


def arm_series(reports, arm):
    """The median frame of each round, for one arm."""
    out = []
    for report in reports:
        for entry in report.get("extra", {}).get("arms", []):
            if entry["arm"] == arm:
                out.extend(entry["round_p50_ns"])
    return out


def bootstrap_median(values):
    """A 95% interval for the median, by resampling."""
    if not values:
        return None
    rng = random.Random(BOOTSTRAP_SEED)
    n = len(values)
    medians = []
    for _ in range(BOOTSTRAP_RESAMPLES):
        sample = [values[rng.randrange(n)] for _ in range(n)]
        medians.append(statistics.median(sample))
    medians.sort()
    lo = medians[int(0.025 * len(medians))]
    hi = medians[min(len(medians) - 1, int(0.975 * len(medians)))]
    return [lo, hi]


def paired(a, b):
    """b minus a, round by round, over the rounds both have."""
    return [y - x for x, y in zip(a, b)]


def pooled_dist(reports, arm, key):
    """Every pooled distribution for one arm, one per report."""
    return [
        entry[key]
        for report in reports
        for entry in report.get("extra", {}).get("arms", [])
        if entry["arm"] == arm
    ]


def phases(reports):
    """Per-phase medians pooled over every round that recorded them."""
    by_phase = {}
    for report in reports:
        for entry in report.get("extra", {}).get("phases", []):
            row = by_phase.setdefault(
                entry["phase"], {"phase": entry["phase"], "p50_ns": [], "p99_ns": [], "share_permille": []}
            )
            row["p50_ns"].append(entry["per_frame_ns"]["p50"])
            row["p99_ns"].append(entry["per_frame_ns"]["p99"])
            row["share_permille"].append(entry["share_permille"])
    out = []
    for row in by_phase.values():
        out.append(
            {
                "phase": row["phase"],
                "median_p50_ns": statistics.median(row["p50_ns"]),
                "median_p99_ns": statistics.median(row["p99_ns"]),
                "median_share_percent": statistics.median(row["share_permille"]) / 10.0,
            }
        )
    return out


def compare(run_dir):
    absent_reports = load_rounds(run_dir, "absent")
    probe_reports = load_rounds(run_dir, "probe")
    absent = arm_series(absent_reports, "absent")
    off = arm_series(probe_reports, "off")
    on = arm_series(probe_reports, "on")

    if not absent or not off:
        raise SystemExit(
            f"{run_dir} has no paired rounds: found {len(absent)} absent and {len(off)} off"
        )

    off_minus_absent = paired(absent, off)
    on_minus_off = paired(off, on)
    # The same arm against itself, one round apart. This is the smallest
    # difference the method can distinguish from drift on this machine.
    noise = [abs(d) for d in paired(absent[:-1], absent[1:])] or [0]

    diff_ci = bootstrap_median(off_minus_absent)
    noise_ceiling = max(noise)
    verdict = abs(statistics.median(off_minus_absent)) <= noise_ceiling

    failures = []
    for report in absent_reports + probe_reports:
        failures.extend(report.get("failures", []))

    return {
        "run": os.path.basename(os.path.abspath(run_dir)),
        "rounds": min(len(absent), len(off)),
        "arms": {
            "absent_p50_ns": absent,
            "off_p50_ns": off,
            "on_p50_ns": on,
        },
        "off_minus_absent_ns": {
            "per_round": off_minus_absent,
            "median": statistics.median(off_minus_absent),
            "bootstrap_95": diff_ci,
        },
        "on_minus_off_ns": {
            "per_round": on_minus_off,
            "median": statistics.median(on_minus_off) if on_minus_off else None,
        },
        "noise_ns": {
            "absent_against_absent": noise,
            "ceiling": noise_ceiling,
        },
        "zero_cost_when_off": verdict,
        "idle_frame_ns": {
            "absent": [d["p50"] for d in pooled_dist(absent_reports, "absent", "idle_ns")],
            "off": [d["p50"] for d in pooled_dist(probe_reports, "off", "idle_ns")],
            "on": [d["p50"] for d in pooled_dist(probe_reports, "on", "idle_ns")],
        },
        "phases": phases(probe_reports),
        "failures": failures,
    }


def text(result):
    lines = []
    lines.append(f"rounds {result['rounds']}")
    for name, key in (
        ("off - absent", "off_minus_absent_ns"),
        ("on  - off   ", "on_minus_off_ns"),
    ):
        entry = result[key]
        lines.append(f"{name}  median {entry['median']} ns  per round {entry['per_round']}")
    lines.append(f"noise ceiling (absent vs absent)  {result['noise_ns']['ceiling']} ns")
    lines.append(
        "zero cost when off: "
        + ("yes" if result["zero_cost_when_off"] else "NO, the difference exceeds the noise band")
    )
    lines.append("")
    lines.append("phase     p50        p99        share")
    for row in result["phases"]:
        lines.append(
            f"{row['phase']:<9} {row['median_p50_ns']:<10.0f} {row['median_p99_ns']:<10.0f} "
            f"{row['median_share_percent']:.1f}%"
        )
    if result["failures"]:
        lines.append("")
        lines.append("failures:")
        lines.extend(f"  {f}" for f in result["failures"])
    return "\n".join(lines)


def main():
    args = [a for a in sys.argv[1:] if a != "--text"]
    if len(args) != 1:
        raise SystemExit(__doc__)
    result = compare(args[0])
    if "--text" in sys.argv[1:]:
        print(text(result))
    else:
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
