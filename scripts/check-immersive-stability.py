#!/usr/bin/env python3
"""Check a stationary immersive session for detail loading and allocation churn."""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
from pathlib import Path
import re
import sys


def parse_timestamp(line: str) -> datetime:
    return datetime.fromisoformat(line.split()[0].replace("Z", "+00:00"))


def check_log(
    text: str,
    *,
    seconds: float = 30.0,
    minimum_dem_zoom: int = 11,
) -> str:
    lines = text.splitlines()
    starts = [i for i, line in enumerate(lines) if "creating the map width=" in line]
    if starts:
        lines = lines[starts[-1] :]
    rows = []
    for line in lines:
        if "resident tiles tiles=" not in line:
            continue
        fields = dict(re.findall(r"(\w+)=(\S+)", line))
        rows.append((parse_timestamp(line), fields))
    if not rows:
        raise ValueError("FAIL: no renderer residency reports")
    end = rows[-1][0]
    window = [row for row in rows if (end - row[0]).total_seconds() <= seconds + 2]
    if len(window) < 10 or (window[-1][0] - window[0][0]).total_seconds() < seconds:
        raise ValueError("FAIL: insufficient stationary observation time")
    poses = []
    for line in lines:
        if "map view frame=" in line:
            time = parse_timestamp(line)
            if window[0][0] <= time <= end:
                poses.append(dict(re.findall(r"(\w+)=(\S+)", line)))
    if len(poses) < 2:
        raise ValueError("FAIL: insufficient camera pose observations")
    for field in ["zoom", "center_latitude", "center_longitude", "pitch"]:
        values = [float(pose[field]) for pose in poses]
        if max(values) - min(values) > 1e-6:
            raise ValueError(f"FAIL: camera {field} moved during the observation")
    for field in ["pool_revision", "symbol_revision"]:
        values = {row[1].get(field) for row in window}
        if None in values or len(values) != 1:
            raise ValueError(f"FAIL: {field} changed during the stationary window: {values}")
    for _, row in window:
        if row.get("pending") != "0" or row.get("settled") != "true":
            raise ValueError("FAIL: the session has not finished loading")
        if int(row.get("finest_dem_zoom", 0)) < minimum_dem_zoom:
            raise ValueError("FAIL: detailed terrain did not load")
    if any("ERROR" in line or "stereo frame failed" in line for line in lines):
        raise ValueError("FAIL: session contains renderer failures")
    duration = (window[-1][0] - window[0][0]).total_seconds()
    return (
        f"PASS: {len(window)} reports over {duration:.1f}s; "
        f"vector/symbol allocations stable; DEM zoom {window[-1][1]['finest_dem_zoom']}"
    )


def synthetic_passing_log() -> str:
    start = datetime(2026, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
    lines = ["2026-01-01T12:00:00Z INFO creating the map width=2732 height=2048"]
    for step in range(12):
        ts = (start + timedelta(seconds=step * 3)).isoformat().replace("+00:00", "Z")
        lines.append(
            f"{ts} INFO map view frame={step} zoom=12.500000 "
            "center_latitude=47.260200 center_longitude=11.344000 pitch=55.000000"
        )
        lines.append(
            f"{ts} INFO resident tiles tiles=24 pending=0 settled=true "
            "pool_revision=4 symbol_revision=2 finest_dem_zoom=12"
        )
    return "\n".join(lines) + "\n"


def run_self_test() -> int:
    summary = check_log(synthetic_passing_log())
    if not summary.startswith("PASS:"):
        sys.stderr.write(f"Unexpected self-test output: {summary}\n")
        return 1
    sys.stdout.write(f"Self-test {summary}\n")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path, nargs="?")
    parser.add_argument("--seconds", type=float, default=30)
    parser.add_argument("--minimum-dem-zoom", type=int, default=11)
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Validate check_log against an internal stationary session fixture",
    )
    args = parser.parse_args(argv)
    if args.self_test:
        return run_self_test()
    if args.log is None:
        parser.error("the following arguments are required: log (unless --self-test is set)")
    try:
        summary = check_log(
            args.log.read_text(),
            seconds=args.seconds,
            minimum_dem_zoom=args.minimum_dem_zoom,
        )
    except ValueError as error:
        raise SystemExit(str(error)) from None
    print(summary)
    return 0


def __getattr__(name: str):
    if name == "test":
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            f"{__name__}.test", Path(__file__).with_name("check-immersive-stability.test.py")
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


if __name__ == "__main__":
    raise SystemExit(main())
