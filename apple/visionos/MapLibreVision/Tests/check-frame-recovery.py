#!/usr/bin/env python3
"""Validate a simulator log captured with --simulate-render-failure-once."""
import argparse
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("log", type=Path)
args = parser.parse_args()
lines = args.log.read_text().splitlines()
injected = [i for i, line in enumerate(lines) if "simulated renderer failure; preserving stereo frame" in line]
if len(injected) != 1:
    raise SystemExit(f"Expected one exercised failure, found {len(injected)}")
following = lines[injected[0] + 1:]
if not following or "stereo frame failed; presenting the last complete frame" not in following[0]:
    raise SystemExit("The failed frame did not select stereo recovery")
reports = [line for line in following if "frame time ms: total" in line]
if len(reports) < 5:
    raise SystemExit(f"Only {len(reports)} frame reports after failure; need five to establish continued presentation")
if any("ERROR" in line or "stereo frame failed" in line for line in following[1:]):
    raise SystemExit("Rendering did not recover cleanly after the injected failure")
print(f"PASS: injected failure recovered; {len(reports)} subsequent frame reports")
