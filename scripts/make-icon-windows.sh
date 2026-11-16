#!/usr/bin/env bash
# Regenerate packaging/windows/icon.ico from packaging/macos/icon.svg.
# Needs Python 3 with Pillow and cairosvg: pip install Pillow cairosvg
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
svg="$root/packaging/macos/icon.svg"
out="$root/packaging/windows/icon.ico"

mkdir -p "$(dirname "$out")"

python3 - "$svg" "$out" <<'PY'
import sys, io, os
import cairosvg
from PIL import Image

svg, out = sys.argv[1], sys.argv[2]
sizes = [16, 32, 48, 64, 128, 256]
images = []
for size in sizes:
    png = cairosvg.svg2png(url=svg, output_width=size, output_height=size)
    images.append(Image.open(io.BytesIO(png)).convert("RGBA"))

# Save largest as the main image, rest as append_images.
images[-1].save(out, format="ICO", append_images=images[:-1])
print(f"Wrote {out} ({os.path.getsize(out)} bytes)")
PY
