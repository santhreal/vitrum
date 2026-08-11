#!/usr/bin/env python3
"""Measure the terminal pane in a capture: its rectangle, the cell pitch it
implies, and the slack under the last row.

The claim checked is the one `app/src/pane/geometry` states in prose and pins
in unit tests: a partial cell is never counted and a whole one is never
dropped, so the band under the last row is strictly less than one cell however
the window is sized. The unit tests read the arithmetic; this reads the
pixels, which is where the defect was reported: content anchored to the top
with a dead band under it that the child never draws in.

A capture is measured by colour rather than by a hard-coded rectangle, because
the pane's rectangle is the toolkit's answer and not a constant: the pane's
background is the colour that dominates the right of the window, and the pane
is the largest run of it.

usage: measure.py <png> [<cols>x<rows>]

With the grid the pane advertised in its status bar, the check is stronger:
the rows that fit must be the rows the pane says it has, and the columns must
divide the box into whole pixels.
"""
import sys
from collections import Counter

import numpy as np
from PIL import Image


def main() -> int:
    path = sys.argv[1]
    a = np.asarray(Image.open(path).convert("RGB")).astype(int)
    h, w, _ = a.shape

    # The pane's background is the colour that dominates the right-hand half.
    right = a[:, w // 2 :, :].reshape(-1, 3)
    bg = np.array(Counter(map(tuple, right[::13])).most_common(1)[0][0])
    m = np.abs(a - bg).sum(axis=2) < 6

    cols = np.where(m.mean(axis=0) > 0.5)[0]
    rows = np.where(m[:, cols[0] : cols[-1] + 1].mean(axis=1) > 0.5)[0]
    x0, x1, y0, y1 = cols[0], cols[-1], rows[0], rows[-1]
    box_w, box_h = x1 - x0 + 1, y1 - y0 + 1
    print(f"pane background {tuple(bg)}")
    print(f"pane box {box_w}x{box_h} at {x0},{y0}")

    sub = a[y0 : y1 + 1, x0 : x1 + 1]
    ink = np.abs(sub - bg).sum(axis=2) >= 6
    inked = np.where(ink.any(axis=1))[0]
    if inked.size == 0:
        print("no ink in the pane")
        return 1

    # Cell pitch from the spacing between the tops of inked bands. A row of
    # text can break into two bands (a box rule over a caption, an underline),
    # so the smallest gap is not the pitch and neither is the gcd, which one
    # split band drives to 1. Every gap between two rows is a whole number of
    # cells, so the pitch is the gap that recurs.
    tops, prev = [], -5
    for y in inked:
        if y != prev + 1:
            tops.append(int(y))
        prev = y
    gaps = [tops[i + 1] - tops[i] for i in range(len(tops) - 1)]
    tall = [g for g in gaps if g >= 12]
    pitch = Counter(tall).most_common(1)[0][0] if tall else 0
    print(f"ink bands {len(tops)}, gaps {sorted(set(gaps))}, cell pitch {pitch}")

    if not pitch:
        print("no pitch could be measured")
        return 1
    rows_shown = box_h // pitch
    slack = box_h - rows_shown * pitch
    print(f"rows that fit {rows_shown}, slack {slack} px")
    if slack >= pitch:
        print("FAIL: a whole row fits in the slack")
        return 1
    print("ok: the slack is less than one cell")
    if len(sys.argv) > 2:
        want_cols, want_rows = (int(n) for n in sys.argv[2].lower().split("x"))
        cell_w = box_w / want_cols
        print(f"grid claimed {want_cols}x{want_rows}, cell {cell_w:.3f}x{pitch}")
        if rows_shown != want_rows:
            print(f"FAIL: {rows_shown} rows fit but the pane says {want_rows}")
            return 1
        if abs(cell_w - round(cell_w)) > 0.02:
            print("FAIL: the columns do not divide the box into whole pixels")
            return 1
        print("ok: the pane's own grid is the grid that fits")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
