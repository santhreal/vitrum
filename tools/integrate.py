#!/usr/bin/env python3
"""Batch integration for the canonical tree.

The flow this supports, and the reason each step exists:

    plan      group open PRs into waves that touch disjoint files
    stage     merge one wave onto a staging branch, one PR at a time
    gate      build and test the staged result once, for the whole wave
    attribute when a wave is red, find which PR made it red
    land      fast-forward main to the staged result and push
    lanes     show every worktree, what is uncommitted in it, and what is unpushed

Two rules are enforced here rather than remembered:

Nothing is ever squashed. A wave keeps one merge commit per pull request, so
`git bisect` still lands on the pull request that broke something. Squashing a
wave would collapse ten reviewed changes into one commit that bisect can only
point at as a whole, which is the entire cost of batching and the one part of
it that cannot be undone later.

A wave is landed only as a descendant of the tip it was staged from. If main
moved while a wave was being reviewed, the wave is restaged rather than merged
on top, so what gets gated and what gets landed are the same tree.

The gate is a build with warnings fatal and then the full test run. Override it
with VITRUM_GATE if a host needs different flags; the default is what works on
a machine with the workspace already fetched.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GATE = os.environ.get(
    "VITRUM_GATE",
    "cargo build --release --workspace --offline"
    " && cargo test --release --workspace --offline",
)


def run(*args: str, cwd: Path | None = None, check: bool = True) -> str:
    done = subprocess.run(
        args, cwd=cwd or ROOT, capture_output=True, text=True, check=False
    )
    if check and done.returncode != 0:
        sys.exit(f"{' '.join(args)}\n{done.stdout}{done.stderr}")
    return (done.stdout + done.stderr).strip()


def git(*args: str, cwd: Path | None = None, check: bool = True) -> str:
    return run("git", *args, cwd=cwd, check=check)


def current_branch() -> str:
    return git("rev-parse", "--abbrev-ref", "HEAD")


def pr_ref(number: int) -> str:
    return f"refs/remotes/pr/{number}"


def fetch_prs() -> None:
    """Refresh every pull request head under refs/remotes/pr/*."""
    git("fetch", "origin", "refs/pull/*/head:refs/remotes/pr/*", "--force")


def pr_files(number: int) -> set[str]:
    """The files a pull request changes, against where it forked from."""
    base = git("merge-base", "main", pr_ref(number))
    listing = git("diff", "--name-only", base, pr_ref(number))
    return {line for line in listing.splitlines() if line}


# A lock file conflict is regenerated from the manifests, never resolved by
# choosing a side, so two pull requests touching only this are not in conflict
# in the sense that matters for grouping. Without this every dependency bump
# lands in a wave of its own and batching buys nothing.
REGENERABLE = {"Cargo.lock"}


def already_in(number: int) -> bool:
    """Whether main already contains this pull request's head."""
    done = subprocess.run(
        ["git", "merge-base", "--is-ancestor", pr_ref(number), "main"],
        cwd=ROOT, capture_output=True, text=True,
    )
    return done.returncode == 0


def open_prs(limit: int) -> list[int]:
    raw = run(
        "gh", "pr", "list", "--state", "open", "--limit", str(limit),
        "--json", "number", "--jq", ".[].number",
    )
    return sorted(int(n) for n in raw.split())


def cmd_plan(args: argparse.Namespace) -> int:
    """Group open pull requests into waves that do not touch the same file.

    Two changes to one file are not necessarily a conflict, but they are the
    only thing that can be one, and a wave whose members cannot conflict is a
    wave whose failure is attributable without bisecting.
    """
    fetch_prs()
    numbers = [int(n) for n in args.prs] if args.prs else open_prs(args.limit)
    touched: dict[int, set[str]] = {}
    for number in numbers:
        try:
            if already_in(number):
                print(f"  pr {number}: already in main, skipped")
                continue
            touched[number] = pr_files(number) - REGENERABLE
        except SystemExit:
            print(f"  pr {number}: no fetched head, skipped")

    waves: list[list[int]] = []
    claimed: list[set[str]] = []
    for number in sorted(touched, key=lambda n: (len(touched[n]), n)):
        files = touched[number]
        for index, taken in enumerate(claimed):
            if not (files & taken):
                waves[index].append(number)
                taken |= files
                break
        else:
            waves.append([number])
            claimed.append(set(files))

    for index, wave in enumerate(waves, 1):
        print(f"wave {index}: {' '.join(f'#{n}' for n in wave)}")
        for number in wave:
            names = sorted(touched[number])
            head = ", ".join(names[:3]) + (" ..." if len(names) > 3 else "")
            print(f"    #{number:<4} {len(names):>3} files  {head}")
    if waves:
        print(f"\nstage the first wave: tools/integrate.py stage {' '.join(str(n) for n in waves[0])}")
    return 0


def merged_into(ref: str, branch: str) -> bool:
    """Whether `ref` is already an ancestor of `branch`."""
    done = subprocess.run(
        ["git", "merge-base", "--is-ancestor", ref, branch],
        cwd=ROOT, capture_output=True, text=True,
    )
    return done.returncode == 0


def branch_exists(name: str) -> bool:
    return bool(name) and subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", name],
        cwd=ROOT, capture_output=True, text=True,
    ).returncode == 0


def save_wave(branch: str, base: str, landed: list[int], requested: list[int]) -> None:
    """Record the wave, including a partial one stopped by a conflict.

    Written before the conflict is resolved, not after the wave completes, or a
    resolution has nothing to resume onto and the next run starts over.
    """
    (ROOT / ".git" / "vitrum-wave").write_text(
        json.dumps(
            {"branch": branch, "base": base, "prs": landed, "requested": requested}
        )
        + "\n"
    )


def cmd_stage(args: argparse.Namespace) -> int:
    """Merge each pull request onto a fresh staging branch, one at a time."""
    fetch_prs()
    numbers = [int(n) for n in args.prs]
    base = args.base
    branch = args.name or f"staging/{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}"

    state_path = ROOT / ".git" / "vitrum-wave"
    dirty = [
        line[3:] for line in git("status", "--porcelain").splitlines() if line
    ]
    incoming: set[str] = set()
    for number in numbers:
        incoming |= pr_files(number)
    clash = sorted(set(dirty) & incoming)
    if clash:
        sys.exit(
            "these files are uncommitted here and are also changed by the wave:\n  "
            + "\n  ".join(clash)
            + "\ncommit or move them before staging"
        )
    if dirty:
        print(f"note: {len(dirty)} uncommitted file(s) untouched by this wave, left alone")

    # Replay a conflict resolution that has already been made once. Restaging a
    # wave re-merges every pull request in it, so without this the same conflict
    # is resolved by hand on every attempt, and a wave that conflicts twice
    # costs the same work twice.
    git("config", "rerere.enabled", "true")

    # Continue a wave a conflict stopped rather than starting a new branch and
    # throwing the resolution away. The same command that stopped resumes it,
    # which is what the message at the bottom of this function promises.
    prior = json.loads(state_path.read_text()) if state_path.is_file() else {}
    resuming = (
        prior.get("requested") == numbers
        and prior.get("base") == base
        and branch_exists(prior.get("branch", ""))
    )
    if resuming:
        branch = prior["branch"]
        git("checkout", branch)
        print(f"resuming {branch}, {len(prior['prs'])} already merged")
    else:
        git("checkout", "-b", branch, base)
        print(f"staging {branch} from {base}")
    landed: list[int] = []
    for number in numbers:
        if merged_into(pr_ref(number), branch):
            landed.append(number)
            print(f"  #{number}: already on this branch")
            continue
        # --no-ff always: the merge commit is what bisect points at later, and
        # a fast-forward would erase which pull request a change arrived in.
        done = subprocess.run(
            ["git", "merge", "--no-ff", "--no-edit", "-m",
             f"Merge pull request #{number}", pr_ref(number)],
            cwd=ROOT, capture_output=True, text=True,
        )
        if done.returncode != 0:
            conflicts = git("diff", "--name-only", "--diff-filter=U")
            print(f"  #{number}: CONFLICT\n    " + "\n    ".join(conflicts.splitlines()))
            save_wave(branch, base, landed, numbers)
            print("resolve, `git add` the files, `git commit`, then run the same"
                  " stage command again to continue on this branch")
            return 1
        landed.append(number)
        print(f"  #{number}: merged")

    save_wave(branch, base, landed, numbers)
    print(f"\n{len(landed)} merged onto {branch}\nnext: tools/integrate.py gate")
    return 0


def wave_state() -> dict:
    path = ROOT / ".git" / "vitrum-wave"
    if not path.is_file():
        sys.exit("no staged wave; run stage first")
    return json.loads(path.read_text())


def run_gate() -> bool:
    print(f"$ {GATE}")
    done = subprocess.run(GATE, cwd=ROOT, shell=True)
    return done.returncode == 0


def cmd_gate(_: argparse.Namespace) -> int:
    state = wave_state()
    if current_branch() != state["branch"]:
        git("checkout", state["branch"])
    ok = run_gate()
    print("gate:", "green" if ok else "RED")
    if not ok:
        print("attribute it: tools/integrate.py attribute")
    return 0 if ok else 1


def cmd_attribute(_: argparse.Namespace) -> int:
    """Re-gate the wave one pull request at a time to find the one that broke it.

    Only worth running when a wave is red. Each step gates the wave truncated
    to its first N merges, so the first red N names the pull request, and the
    cost is one gate per pull request instead of one per pull request always.
    """
    state = wave_state()
    prs = state["prs"]
    git("checkout", "-B", "staging/attribute", state["base"])
    for number in prs:
        git("merge", "--no-ff", "--no-edit", "-m", f"Merge pull request #{number}",
            pr_ref(number))
        print(f"--- gating through #{number} ---")
        if not run_gate():
            print(f"\nfirst red at #{number}")
            return 1
        print(f"#{number}: green")
    print("\nevery prefix is green; the wave is red only as a whole")
    return 0


def cmd_land(args: argparse.Namespace) -> int:
    """Move main to the gated staging branch, keeping every merge commit."""
    state = wave_state()
    branch = state["branch"]
    if git("merge-base", "--is-ancestor", "main", branch, check=False) != "":
        sys.exit(
            f"main is not an ancestor of {branch}: it moved while the wave was staged.\n"
            "restage the wave on the new main rather than merging on top, so what was\n"
            "gated and what lands are the same tree"
        )
    git("checkout", "main")
    # --ff-only: the wave already contains one merge commit per pull request,
    # so this moves main onto them without inventing an eleventh commit that
    # describes nothing. Nothing is squashed.
    git("merge", "--ff-only", branch)
    print(git("log", "--oneline", "--first-parent", f"{state['base']}..HEAD"))
    if args.push:
        print(git("push", "origin", "main"))
    (ROOT / ".git" / "vitrum-wave").unlink(missing_ok=True)
    return 0


def cmd_lanes(_: argparse.Namespace) -> int:
    """Every worktree, what is uncommitted in it, and what it has that main does not.

    Commits survive a worktree being deleted, because the refs live in the
    shared repository. Uncommitted work does not. This exists so that the
    difference is one command rather than an audit.
    """
    rows = []
    for line in git("worktree", "list").splitlines():
        path = Path(line.split()[0])
        if not path.is_dir():
            rows.append((str(path), "GONE", "-", "-", "-"))
            continue
        branch = git("rev-parse", "--abbrev-ref", "HEAD", cwd=path)
        dirty = len([l for l in git("status", "--porcelain", cwd=path).splitlines() if l])
        ahead = len(git("log", "--oneline", f"main..{branch}", cwd=path,
                        check=False).splitlines())
        upstream = git("rev-parse", "--abbrev-ref", f"{branch}@{{upstream}}",
                       cwd=path, check=False)
        unpushed = (
            len(git("log", "--oneline", f"{upstream}..{branch}", cwd=path,
                    check=False).splitlines())
            if "@{upstream}" not in upstream and upstream else "no upstream"
        )
        rows.append((str(path), branch, dirty, ahead, unpushed))

    width = max(len(r[0]) for r in rows)
    print(f"{'worktree':<{width}}  {'branch':<34} {'dirty':>5} {'unmerged':>8}  unpushed")
    for path, branch, dirty, ahead, unpushed in rows:
        volatile = " (/tmp: uncommitted work here dies on reboot)" if path.startswith("/tmp") else ""
        print(f"{path:<{width}}  {branch:<34} {dirty:>5} {ahead:>8}  {unpushed}{volatile}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)

    plan = sub.add_parser("plan", help="group open PRs into non-overlapping waves")
    plan.add_argument("prs", nargs="*", help="limit to these PR numbers")
    plan.add_argument("--limit", type=int, default=60)
    plan.set_defaults(fn=cmd_plan)

    stage = sub.add_parser("stage", help="merge a wave onto a staging branch")
    stage.add_argument("prs", nargs="+")
    stage.add_argument("--base", default="main")
    stage.add_argument("--name")
    stage.set_defaults(fn=cmd_stage)

    gate = sub.add_parser("gate", help="build and test the staged wave")
    gate.set_defaults(fn=cmd_gate)

    attribute = sub.add_parser("attribute", help="find which PR made a wave red")
    attribute.set_defaults(fn=cmd_attribute)

    land = sub.add_parser("land", help="move main onto the gated wave")
    land.add_argument("--push", action="store_true")
    land.set_defaults(fn=cmd_land)

    lanes = sub.add_parser("lanes", help="worktree, dirty and unpushed audit")
    lanes.set_defaults(fn=cmd_lanes)

    args = parser.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
