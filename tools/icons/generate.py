"""Generates the application icons in assets/icons from vector shapes.

Requires Pillow. Run from the repository root:

    python tools/icons/generate.py

Each icon is drawn at 1024 px and downsampled for the sizes Windows uses, so the
shapes stay sharp at 16 px. Colors follow the editors' theme (aud-ui).
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

CANVAS = 1024
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
OUT = Path("assets/icons")

BACKGROUND_TOP = (0x22, 0x26, 0x2F)
BACKGROUND_BOTTOM = (0x12, 0x14, 0x19)
BORDER = (0x3A, 0x40, 0x4D)
SEND = (0xFF, 0x8A, 0x3D)
RECEIVE = (0x3C, 0xD6, 0xC0)
REMOVED = (0xFF, 0x5C, 0x7A)


def background() -> Image.Image:
    """Rounded square with a vertical gradient and a thin border."""
    gradient = Image.new("RGBA", (CANVAS, CANVAS))
    draw = ImageDraw.Draw(gradient)
    for y in range(CANVAS):
        t = y / (CANVAS - 1)
        color = tuple(
            round(top + (bottom - top) * t)
            for top, bottom in zip(BACKGROUND_TOP, BACKGROUND_BOTTOM)
        )
        draw.line([(0, y), (CANVAS, y)], fill=color + (255,))
    inset = CANVAS * 0.04
    box = [inset, inset, CANVAS - inset, CANVAS - inset]
    radius = CANVAS * 0.2
    mask = Image.new("L", (CANVAS, CANVAS), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius, fill=255)
    icon = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    icon.paste(gradient, mask=mask)
    ImageDraw.Draw(icon).rounded_rectangle(box, radius, outline=BORDER + (255,), width=12)
    return icon


def with_alpha(color, alpha):
    return color + (round(255 * alpha),)


def signal(color, facing_right: bool) -> Image.Image:
    """A dot with three arcs: going out to the right (transmit) or coming in from the
    left (receive)."""
    icon = background()
    layer = Image.new("RGBA", icon.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    centre_x = CANVAS * (0.32 if facing_right else 0.68)
    centre_y = CANVAS * 0.5
    dot = CANVAS * 0.085
    draw.ellipse(
        [centre_x - dot, centre_y - dot, centre_x + dot, centre_y + dot],
        fill=color + (255,),
    )
    start, end = (-48, 48) if facing_right else (132, 228)
    for index, radius in enumerate((0.2, 0.31, 0.42)):
        r = CANVAS * radius
        draw.arc(
            [centre_x - r, centre_y - r, centre_x + r, centre_y + r],
            start,
            end,
            fill=with_alpha(color, 1.0 - 0.22 * index),
            width=round(CANVAS * 0.07),
        )
    return Image.alpha_composite(icon, layer)


def stroke(draw: ImageDraw.ImageDraw, points, width: float, color) -> None:
    """A polyline with round joins and caps."""
    fill = color + (255,)
    draw.line(points, fill=fill, width=round(width))
    r = width / 2
    for x, y in points:
        draw.ellipse([x - r, y - r, x + r, y + r], fill=fill)


def cleanup() -> Image.Image:
    """O-NEC: a noisy trace on the left that becomes a clean sine on the right."""
    icon = background()
    layer = Image.new("RGBA", icon.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    left, right = CANVAS * 0.17, CANVAS * 0.83
    middle = CANVAS * 0.5
    amplitude = CANVAS * 0.2
    width = CANVAS * 0.065

    def sine(t: float) -> float:
        return middle - amplitude * math.sin(2.0 * math.pi * 1.25 * t)

    def x_of(t: float) -> float:
        return left + (right - left) * t

    # Spikes around the sine, shrinking towards the clean part.
    spikes = [0.0, 0.55, -0.6, 0.65, -0.45, 0.5, -0.3, 0.2, 0.0]
    noisy = [
        (x_of(0.45 * index / (len(spikes) - 1)),
         sine(0.45 * index / (len(spikes) - 1)) + spike * amplitude)
        for index, spike in enumerate(spikes)
    ]
    clean = [(x_of(t), sine(t)) for t in (0.45 + 0.55 * step / 120 for step in range(121))]
    stroke(draw, noisy, width * 0.6, REMOVED)
    stroke(draw, clean, width, RECEIVE)
    return Image.alpha_composite(icon, layer)


def save(name: str, image: Image.Image) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    image.resize((256, 256), Image.LANCZOS).save(OUT / f"{name}.png")
    frames = [image.resize((size, size), Image.LANCZOS) for size in ICO_SIZES]
    frames[-1].save(
        OUT / f"{name}.ico",
        format="ICO",
        sizes=[(size, size) for size in ICO_SIZES],
        append_images=frames[:-1],
    )


def main() -> None:
    save("o-aud-tx", signal(SEND, facing_right=True))
    save("o-aud-rx", signal(RECEIVE, facing_right=False))
    save("o-nec", cleanup())


if __name__ == "__main__":
    main()
