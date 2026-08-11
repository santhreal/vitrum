# Appearance

Settings › Appearance.

| Control | Effect |
|---|---|
| Terminal palette | the sixteen ANSI slots and the grid's own background |
| Window opacity | the whole window, chrome included |
| Terminal opacity | the grid alone |
| Backdrop | an image drawn inside the window, with fit, blur and dim |

Both opacity controls default to fully opaque and emit no CSS, so an install
that never opens this tab composites nothing.

## Terminal palette

The palette is independent of light and dark. Choosing one moves no chrome.

| Palette | Setting value |
|---|---|
| Follow the app theme | `inherit` |
| Solarized Dark | `solarized-dark` |
| Solarized Light | `solarized-light` |
| Gruvbox Dark | `gruvbox-dark` |
| Nord | `nord` |
| Dracula | `dracula` |
| Tokyo Night | `tokyo-night` |
| One Half Light | `one-half-light` |

Every entry is a palette with a published definition that predates this
product. The value of a named palette is that you already know what it looks
like, so none is invented here.

`inherit` is the default and is not a palette. It declares nothing and lets the
app theme's own terminal colours through.

One palette reaches two painters: the renderer draws the cell matrix from it,
and the letterboxing around the matrix is styled from the same numbers, so the
area outside the grid is never a different black from the cells inside it.

## Your own terminal's colours

Turn on "Follow the host terminal" to paint the grid with the colours your
terminal is already configured with, instead of with a built-in scheme.

The import reads a configuration file and parses it. Four formats are
understood:

| Format | Terminals that write it |
|---|---|
| sectioned key/value | alacritty, foot |
| flat `key value` | kitty |
| X resources, `*color0` to `*color15` | anything reading `.Xresources` |
| JSON scheme list | Windows Terminal |

Candidates, in the order they are tried:

1. `$XDG_CONFIG_HOME/alacritty/alacritty.toml`
2. `$XDG_CONFIG_HOME/kitty/kitty.conf`
3. `$XDG_CONFIG_HOME/foot/foot.ini`
4. `~/.Xresources`
5. `~/.Xdefaults`
6. Windows Terminal's `settings.json` under `AppData\Local\Packages`

`XDG_CONFIG_HOME` falls back to `~/.config`. A terminal that exports a variable
naming itself moves its own file to the front of that list: `KITTY_WINDOW_ID`,
`ALACRITTY_WINDOW_ID`, `ALACRITTY_SOCKET`, `WT_SESSION`, or a `TERM` containing
`alacritty` or `foot`. Every other candidate is still tried afterwards, because
running one terminal and configuring another is ordinary.

The first file that yields all twenty colours wins: sixteen ANSI slots, a
background, a foreground, a cursor and a selection. A file that yields some of
them is refused rather than merged, and the error names each file it read and
what that file was missing. A cursor or selection colour that is absent falls
back to the foreground.

The result is stored, not re-detected each launch, so the grid does not change
colour because a configuration file moved. The settings row shows which file
the colours came from. Point the import at a specific file when the scan does
not know your terminal; the format is decided by the file's shape rather than
by its name.

What it cannot read:

- Colours set at run time with OSC 4, 10 or 11. There is no query path back:
  asking needs a controlling terminal in raw mode, and this process has none.
- A configuration that is a program, computed in Lua or chosen per window.
- Colours 16 through 255. Those are the standard colour cube and greyscale
  ramp, identical in every terminal, and not a preference.
- Anything but colour. Font, cursor shape, blink and scrollback are separate
  settings here, because a terminal's font size was chosen for that window.

Started from a launcher rather than from a terminal, this process has no
controlling terminal, so the scan reads whichever file exists rather than the
colours of any window on screen. With more than one terminal installed that is
a guess, and the file it read is shown so the guess is visible.

## Backdrop

The backdrop is drawn inside the window. It looks the same on every platform
and needs nothing from the desktop. Point it at an absolute path to a PNG,
JPEG, GIF or WEBP.

Files are checked by signature, not by extension. SVG is refused: it is a
scripted document and it would render inside the application page.

## Blur

An application cannot blur what is behind its own window. The compositor owns
that, and Wayland has no protocol for an application to ask. vitrum makes the
window transparent; the compositor frosts it.

Lower the window opacity, then add one rule.

Hyprland, in `hyprland.conf`:

```
windowrule = opacity 1.0 override, class:^(vitrum)$
blur = yes                      # under decoration { }
```

picom, in `picom.conf`:

```
blur-background-exclude = [ "class_g != 'vitrum'" ];
```

KWin frosts translucent windows through System Settings › Desktop Effects ›
Blur, with no per-application rule.

With no compositor running, a transparent window has nothing to blend with. Use
a backdrop image instead.

Mica and Acrylic on Windows, and `NSVisualEffectView` on macOS, are not in this
release.

## The mark

<p align="center">
  <img src="../assets/logo/vitrum.svg" alt="The vitrum mark: a cut stone drawn as ten stroked segments on a 96 unit grid" width="96" />
</p>

`assets/logo/vitrum.svg` is the only place the mark's shape is written down.
The window icon, the Windows `.ico`, the macOS `.icns` and the freedesktop
hicolor PNGs are computed from that geometry at build time by `vitrum-os`. No
raster of the mark is committed.
