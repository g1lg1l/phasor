"""Builds the app icon from the logo (needs `pip install pillow`).

    python scripts/icon.py

assets/logo.png is the artwork: the orange raven on the app's dark #171717, square. From it:
- assets/icon.png (512 px): the logo as a rounded tile; the window and taskbar icon (built
  into the app), the macOS .icns (release.yml), the README and the docs site.
- assets/icon.ico (16 to 256 px): the .exe's icon on Windows (build.rs embeds it).
- assets/mark.png (64 px): the raven alone, white on transparent; the app tints it with the
  theme's accent beside the RAVUNE wordmark.
"""
import os

from PIL import Image, ImageDraw

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")


def icon(logo, size):
    """The logo with rounded corners. Masked 4x larger, then scaled down (smooth corners)."""
    big = size * 4
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, big - 1, big - 1], radius=big * 56 // 256, fill=255)
    image = logo.resize((big, big), Image.LANCZOS).convert("RGBA")
    image.putalpha(mask)
    return image.resize((size, size), Image.LANCZOS)


def main():
    logo = Image.open(os.path.join(ROOT, "assets", "logo.png")).convert("RGB")
    icon(logo, 512).save(os.path.join(ROOT, "assets", "icon.png"), optimize=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    # each size made on its own, not scaled from the big one: small ones stay crisp
    images = [icon(logo, s) for s in sizes]
    images[-1].save(os.path.join(ROOT, "assets", "icon.ico"), sizes=[(s, s) for s in sizes], append_images=images[:-1])
    # the mark: how orange each pixel is (tile 0, raven 255), cropped to the raven, square
    alpha = logo.getchannel("R").point(lambda r: max(0, min(255, (r - 23) * 255 // (255 - 23))))
    alpha = alpha.crop(alpha.getbbox())
    side = max(alpha.size)
    square = Image.new("L", (side, side), 0)
    square.paste(alpha, ((side - alpha.width) // 2, (side - alpha.height) // 2))
    mark = Image.new("RGBA", (side, side), (255, 255, 255, 0))
    mark.putalpha(square)
    mark.resize((64, 64), Image.LANCZOS).save(os.path.join(ROOT, "assets", "mark.png"), optimize=True)
    print("wrote assets/icon.png, assets/icon.ico and assets/mark.png")


if __name__ == "__main__":
    main()
