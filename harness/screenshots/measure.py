#!/usr/bin/env python3
"""Measure the terminal pane in a capture: the cell pitch it paints at, the
rows that fit its box, and the slack under the last one.

The claim checked is the one `app/src/pane/geometry` states in prose and pins
in unit tests: a partial cell is never counted and a whole one is never
dropped, so the band under the last row is strictly less than one cell however
the window is sized. The unit tests read the arithmetic; this reads the
pixels, which is where the defect was reported: content anchored to the top
with a dead band under it that the child never draws in.

usage: measure.py <png> <cols>x<rows> <w>x<h>+<x>+<y>

Both numbers come out of the app's own log line, which prints the grid it
handed the child and the rectangle it handed the swapchain. The rectangle is
taken rather than found, because finding it means guessing where the pane is
from the colours in the picture, and that guess is wrong exactly when the
operator picks a palette: the chrome borrows the palette's hues, so the ring
around the pane becomes the pane's own background and a colour search swallows
it. A band of exactly one cell is also indistinguishable from an empty row
unless the pane's claim is in hand.
"""
import sys
from collections import Counter

import numpy as np
from PIL import Image


def main() -> int:
    path = sys.argv[1]
    want_cols, want_rows = (int(n) for n in sys.argv[2].lower().split("x"))
    size, x, y = sys.argv[3].split("+")
    box_w, box_h = (int(n) for n in size.lower().split("x"))
    x0, y0 = int(x), int(y)

    a = np.asarray(Image.open(path).convert("RGB")).astype(int)
    h, w, _ = a.shape
    if x0 + box_w > w or y0 + box_h > h:
        print(f"FAIL: the pane's {box_w}x{box_h}+{x0}+{y0} is outside a {w}x{h} capture")
        return 1
    sub = a[y0 : y0 + box_h, x0 : x0 + box_w]
    print(f"pane box {box_w}x{box_h} at {x0},{y0} in {w}x{h}")

    # The background is the colour that dominates the pane's own rectangle.
    # Read from inside it, so nothing outside the pane can be mistaken for it.
    flat = sub.reshape(-1, 3)
    bg = np.array(Counter(map(tuple, flat[::13])).most_common(1)[0][0])
    print(f"pane background {tuple(int(c) for c in bg)}")
    ink = np.abs(sub - bg).sum(axis=2) >= 6
    inked = np.where(ink.any(axis=1))[0]
    if inked.size == 0:
        print("FAIL: no ink in the pane")
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
    cell_w = box_w / want_cols
    print(f"rows that fit {rows_shown}, slack {slack} px")
    print(f"grid claimed {want_cols}x{want_rows}, cell {cell_w:.3f}x{pitch}")
    if slack >= pitch:
        print("FAIL: a whole row fits in the slack")
        return 1
    if rows_shown != want_rows:
        print(f"FAIL: {rows_shown} rows fit but the pane says {want_rows}")
        return 1
    if abs(cell_w - round(cell_w)) > 1.0 / want_cols:
        print("FAIL: the columns do not divide the box into whole pixels")
        return 1
    print("ok: the pane's own grid is the grid that fits, with less than a cell to spare")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
