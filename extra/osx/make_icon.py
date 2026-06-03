#!/usr/bin/env python3
"""Build a macOS squircle-masked .icns from a source PNG.

Usage: make_icon.py <source.png> <output.icns>

Mirrors Apple's macOS icon grid (Big Sur+): art occupies an 824x824 region
centered inside a 1024x1024 squircle canvas. Requires `iconutil` (macOS) and
Pillow.
"""
import os
import shutil
import subprocess
import sys
import tempfile
from PIL import Image, ImageDraw

CANVAS = 1024
ART = 824       # squircle size inside the canvas (Apple's macOS template)
RADIUS = 181    # corner radius for an 824-wide squircle (~22% of ART)

SIZES = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]


def squircle_mask(size, radius):
    scale = 4
    s = size * scale
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, s - 1, s - 1), radius=radius * scale, fill=255)
    return mask.resize((size, size), Image.LANCZOS)


def build_master(src_path):
    src = Image.open(src_path).convert("RGBA").resize((ART, ART), Image.LANCZOS)
    masked = Image.new("RGBA", (ART, ART), (0, 0, 0, 0))
    masked.paste(src, (0, 0), squircle_mask(ART, RADIUS))
    out = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    offset = (CANVAS - ART) // 2
    out.paste(masked, (offset, offset), masked)
    return out


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: make_icon.py <source.png> <output.icns>")
    src_path, icns_out = sys.argv[1], sys.argv[2]
    master = build_master(src_path)
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "Helide.iconset")
        os.makedirs(iconset)
        for name, size in SIZES:
            master.resize((size, size), Image.LANCZOS).save(os.path.join(iconset, name), "PNG")
        os.makedirs(os.path.dirname(icns_out) or ".", exist_ok=True)
        subprocess.run(["iconutil", "-c", "icns", "-o", icns_out, iconset], check=True)


if __name__ == "__main__":
    main()
