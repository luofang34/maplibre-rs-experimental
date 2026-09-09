#!/usr/bin/env python3
"""Render an offline globe overview with the fork's render-tests binary. Requires Pillow."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
from urllib.request import urlopen
from PIL import Image

COASTLINE = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_50m_land.geojson"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("renderer", type=Path, help="target/debug/render-tests")
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    with urlopen(COASTLINE, timeout=60) as response:
        land = response.read()
    expected = "e874b27a51d146452be360cafb3cc50c86001074a67d534113e6534682f9826b"
    if hashlib.sha256(land).hexdigest() != expected:
        raise ValueError("Natural Earth source changed; review it before rebuilding the atlas")
    with tempfile.TemporaryDirectory(prefix="desk-atlas-") as directory:
        fixture = Path(directory)
        style = dict(version=8, center=[0, 0], zoom=3, width=4096, height=4096, pitch=0, bearing=0,
            projection=dict(type="mercator"),
            sources=dict(land=dict(type="geojson", data=json.loads(land))),
            layers=[dict(id="ocean", type="background", paint={"background-color": "#182e40"}),
                    dict(id="land", type="fill", source="land", paint={"fill-color": "#384650", "fill-antialias": False})])
        (fixture / "style.json").write_text(json.dumps(style))
        # This produces an asset, so there is deliberately no expected comparison image.
        result = subprocess.run([str(args.renderer.resolve()), directory], capture_output=True, text=True)
        rendered = fixture / "actual.png"
        if not rendered.exists():
            raise RuntimeError(f"MapLibre overview rendering failed: {result.stdout}\n{result.stderr}")
        with Image.open(rendered) as image:
            if image.size != (4096, 4096):
                raise ValueError(f"Unexpected atlas size: {image.size}")
            image.resize((2048, 2048), Image.Resampling.LANCZOS).save(args.output, optimize=True)


if __name__ == "__main__":
    main()
