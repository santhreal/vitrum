# The mark

One square pane of glass, cut once on a diagonal, with the two halves slipped
along the cut. The gap between them is the glass edge. That is the whole idea:
*vitrum* is Latin for glass, and glass displaces whatever passes through it.

Provisional. It was accepted as good enough to ship 0.1.0 and is expected to be
refined, so treat the shape as unsettled and the rule below as settled.

## Where it may appear

The mark is allowed in exactly two places:

- **The launcher entry.** The desktop shortcut, the dock, the Start menu tile,
  the macOS bundle icon: whatever the operating system shows before the program
  is running.
- **The loading screen**, if and when one exists. There is none today.

## Where it may not appear

**Nowhere inside the running application.** Not in the titlebar, not in the
sidebar, not in an empty state, not in Settings, not in About, not in a dialog,
not as a watermark, not in a corner at 12px.

The reason is what the window is for. Every pixel of chrome is space not spent
on the thing the operator is actually watching, which is agents doing work. A
logo inside the window tells them something they already know, cannot act on,
and did not ask about. It is the clearest case of the test this product applies
to everything on screen: *what does the operator do differently because this is
here?* Nothing. So it goes.

This is not a stylistic preference to be revisited when someone fancies a
splash of brand. It is a rule.

`app/src/update.rs` carries the test that enforces it, in
`the_mark_stays_out_of_the_window`. It reads the UI sources and fails if the
mark is referenced from any of them. If you are adding the loading screen and
that test fails, extend its allowlist deliberately rather than deleting the
assertion.

## Regenerating

Every file here is generated. There are no coordinates to edit by hand.

```sh
python3 packaging/build-icons.py
```

The mark is three numbers at the top of that script: the cut angle, how far the
halves slip, and the margin. Change those, rerun, and the SVG, every PNG and the
`.ico` all move together. Editing one file by hand is how a logo ends up subtly
different in the dock than on the website.

`.icns` for macOS has to be built on a mac, because `iconutil` ships only there:

```sh
mkdir -p vitrum.iconset
for s in 16 32 128 256 512; do
  cp vitrum-$s.png   vitrum.iconset/icon_${s}x${s}.png
  cp vitrum-$((s*2)).png vitrum.iconset/icon_${s}x${s}@2x.png
done
iconutil -c icns vitrum.iconset
```
