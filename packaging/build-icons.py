#!/usr/bin/env python3
"""Generate every vitrum icon from one description of the mark.

The mark is one square pane of glass cut once on a diagonal, with the two
halves slipped along the cut. The gap between them is the glass edge.

Everything here comes out of the same three numbers below, so the SVG, the
PNGs and the Windows .ico cannot drift from each other. Editing a coordinate in
one file by hand is how a logo ends up subtly different in the dock than on the
website; there is no coordinate to edit here.

Run from the repository root:

    python3 packaging/build-icons.py
"""

import math
import pathlib

# --- the mark, in full -----------------------------------------------------

CUT_DEGREES = 60.0  # angle of the cut, measured from horizontal
SLIP = 12.0         # how far each half moves along the cut normal, at 256px
INSET = 32.0        # margin around the pane, at 256px
BOX = 256.0         # design grid

OUT = pathlib.Path("assets/logo")
PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def clip_to_halfplane(polygon, point, normal):
    """Sutherland-Hodgman against one half-plane: keep what the normal points away from."""

    def inside(p):
        return (p[0] - point[0]) * normal[0] + (p[1] - point[1]) * normal[1] <= 0

    def crossing(a, b):
        da = (a[0] - point[0]) * normal[0] + (a[1] - point[1]) * normal[1]
        db = (b[0] - point[0]) * normal[0] + (b[1] - point[1]) * normal[1]
        t = da / (da - db)
        return (a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]))

    out = []
    for i, current in enumerate(polygon):
        previous = polygon[i - 1]
        if inside(current):
            if not inside(previous):
                out.append(crossing(previous, current))
            out.append(current)
        elif inside(previous):
            out.append(crossing(previous, current))
    return out


def halves(scale=1.0):
    """The two polygons the mark is made of, in a `scale * 256` box."""
    box, inset, slip = BOX * scale, INSET * scale, SLIP * scale
    centre = box / 2
    a = math.radians(CUT_DEGREES)
    normal = (math.sin(a), math.cos(a))  # perpendicular to the cut

    pane = [
        (inset, inset),
        (box - inset, inset),
        (box - inset, box - inset),
        (inset, box - inset),
    ]
    lower = clip_to_halfplane(pane, (centre, centre), normal)
    upper = clip_to_halfplane(pane, (centre, centre), (-normal[0], -normal[1]))

    def move(poly, sign):
        return [(x + normal[0] * slip * sign, y + normal[1] * slip * sign) for x, y in poly]

    return move(lower, -1), move(upper, 1)


def write_svg(path, invert=False):
    background, ink = ("#ffffff", "#000000") if not invert else ("#000000", "#ffffff")
    lower, upper = halves()
    def points(poly):
        return " ".join(f"{x:.2f},{y:.2f}" for x, y in poly)
    path.write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" '
        f'height="256">\n'
        f'  <rect width="256" height="256" fill="{background}"/>\n'
        f'  <polygon points="{points(lower)}" fill="{ink}"/>\n'
        f'  <polygon points="{points(upper)}" fill="{ink}"/>\n'
        f"</svg>\n"
    )


def render(size, transparent=True):
    """Draw at 8x and downsample, which is sharper than any SVG rasteriser at 16px."""
    from PIL import Image, ImageDraw

    supersample = 8
    big = size * supersample
    background = (0, 0, 0, 0) if transparent else (255, 255, 255, 255)
    image = Image.new("RGBA", (big, big), background)
    draw = ImageDraw.Draw(image)
    for polygon in halves(scale=big / BOX):
        draw.polygon(polygon, fill=(0, 0, 0, 255))
    return image.resize((size, size), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    write_svg(OUT / "vitrum.svg")
    write_svg(OUT / "vitrum-inverted.svg", invert=True)

    for size in PNG_SIZES:
        render(size).save(OUT / f"vitrum-{size}.png")

    render(256).save(
        OUT / "vitrum.ico",
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
    )

    print(f"wrote {OUT}/vitrum.svg, {len(PNG_SIZES)} png, vitrum.ico")
    print("macOS .icns needs a mac: iconutil -c icns assets/logo/vitrum.iconset")


if __name__ == "__main__":
    main()
