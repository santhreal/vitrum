# Appearance

Settings › Appearance.

| Control | Effect |
|---|---|
| Window opacity | the whole window, chrome included |
| Terminal opacity | the grid alone |
| Backdrop | an image drawn inside the window, with fit, blur and dim |

Both opacity controls default to fully opaque and emit no CSS, so an install
that never opens this tab composites nothing.

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
