#!/usr/bin/env python3
"""Build a macOS app .icns from a source PNG using Apple's icon template.

Usage: make_icon.py <source.png> <template.png> <output.icns>

The template is Apple's "Template - Icon - App.png" (from
developer.apple.com/design/resources) — a 1024x1024 image with a white
squircle (RGB=255 where the artwork should go) on a transparent background
with the official drop shadow baked in. We paste the source PNG into the
squircle region using the template's R channel as the mask, which preserves
the shadow.
"""
import os
import subprocess
import sys
import tempfile
from PIL import Image

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


def squircle_bbox(template):
    """Bounding box of the opaque-white squircle region in the template."""
    r = template.getchannel("R")
    a = template.getchannel("A")
    # White-and-opaque pixels mark the artwork region
    mask = Image.eval(r, lambda v: 255 if v > 250 else 0)
    mask = Image.eval(Image.merge("LA", (mask, a)).getchannel("L"), lambda v: v)
    return mask.getbbox(), mask


def build_master(src_path, template_path):
    template = Image.open(template_path).convert("RGBA")
    bbox, squircle = squircle_bbox(template)
    x0, y0, x1, y1 = bbox
    art_size = (x1 - x0, y1 - y0)

    src = Image.open(src_path).convert("RGBA").resize(art_size, Image.LANCZOS)

    out = template.copy()
    # Paste source inside the squircle, using the squircle as alpha mask so
    # the artwork is clipped to the squircle shape and the shadow is kept.
    region_mask = squircle.crop(bbox)
    out.paste(src, (x0, y0), region_mask)
    return out


def main():
    if len(sys.argv) != 4:
        sys.exit("usage: make_icon.py <source.png> <template.png> <output.icns>")
    src_path, template_path, icns_out = sys.argv[1:4]
    master = build_master(src_path, template_path)
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "Helide.iconset")
        os.makedirs(iconset)
        for name, size in SIZES:
            master.resize((size, size), Image.LANCZOS).save(os.path.join(iconset, name), "PNG")
        os.makedirs(os.path.dirname(icns_out) or ".", exist_ok=True)
        subprocess.run(["iconutil", "-c", "icns", "-o", icns_out, iconset], check=True)


if __name__ == "__main__":
    main()
