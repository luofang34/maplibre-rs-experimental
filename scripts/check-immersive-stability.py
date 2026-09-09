#!/usr/bin/env python3
"""Check a stationary immersive session for detail loading and allocation churn."""
import argparse
from datetime import datetime
from pathlib import Path
import re

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("log", type=Path)
parser.add_argument("--seconds", type=float, default=30)
parser.add_argument("--minimum-dem-zoom", type=int, default=11)
args = parser.parse_args()
lines = args.log.read_text().splitlines()
starts = [i for i, line in enumerate(lines) if "creating the map width=" in line]
if starts:
    lines = lines[starts[-1]:]
rows = []
for line in lines:
    if "resident tiles tiles=" not in line:
        continue
    fields = dict(re.findall(r"(\w+)=(\S+)", line))
    rows.append((datetime.fromisoformat(line.split()[0].replace("Z", "+00:00")), fields))
if not rows:
    raise SystemExit("FAIL: no renderer residency reports")
end = rows[-1][0]
window = [row for row in rows if (end - row[0]).total_seconds() <= args.seconds + 2]
if len(window) < 10 or (window[-1][0] - window[0][0]).total_seconds() < args.seconds:
    raise SystemExit("FAIL: insufficient stationary observation time")
poses = []
for line in lines:
    if "map view frame=" in line:
        time = datetime.fromisoformat(line.split()[0].replace("Z", "+00:00"))
        if window[0][0] <= time <= end:
            poses.append(dict(re.findall(r"(\w+)=(\S+)", line)))
if len(poses) < 2:
    raise SystemExit("FAIL: insufficient camera pose observations")
for field in ["zoom", "center_latitude", "center_longitude", "pitch"]:
    values = [float(pose[field]) for pose in poses]
    if max(values) - min(values) > 1e-6:
        raise SystemExit(f"FAIL: camera {field} moved during the observation")
for field in ["pool_revision", "symbol_revision"]:
    values = {row[1].get(field) for row in window}
    if None in values or len(values) != 1:
        raise SystemExit(f"FAIL: {field} changed during the stationary window: {values}")
for _, row in window:
    if row.get("pending") != "0" or row.get("settled") != "true":
        raise SystemExit("FAIL: the session has not finished loading")
    if int(row.get("finest_dem_zoom", 0)) < args.minimum_dem_zoom:
        raise SystemExit("FAIL: detailed terrain did not load")
if any("ERROR" in line or "stereo frame failed" in line for line in lines):
    raise SystemExit("FAIL: session contains renderer failures")
print(f"PASS: {len(window)} reports over {(window[-1][0]-window[0][0]).total_seconds():.1f}s; "
      f"vector/symbol allocations stable; DEM zoom {window[-1][1]['finest_dem_zoom']}")
