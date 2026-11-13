#!/usr/bin/env python3
"""Unit tests for scripts/check-immersive-stability.py."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT_PATH = Path(__file__).with_name("check-immersive-stability.py")
SPEC = importlib.util.spec_from_file_location("check_immersive_stability", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def build_log(
    *,
    count: int = 12,
    step_seconds: float = 3.0,
    pose_count: int | None = None,
    drift_zoom: float = 0.0,
    churn_pool: bool = False,
    churn_symbol: bool = False,
    pending: str = "0",
    settled: str = "true",
    dem_zoom: int = 12,
    extra_line: str | None = None,
    earlier_failed_session: bool = False,
) -> str:
    start = datetime(2026, 3, 1, 9, 0, 0, tzinfo=timezone.utc)
    lines: list[str] = []
    if earlier_failed_session:
        lines.extend(
            [
                "2026-03-01T08:50:00Z INFO creating the map width=2732 height=2048",
                "2026-03-01T08:50:05Z ERROR stereo frame failed in previous run",
            ]
        )
    lines.append("2026-03-01T09:00:00Z INFO creating the map width=2732 height=2048")
    total_poses = count if pose_count is None else pose_count
    for step in range(count):
        ts = (start + timedelta(seconds=step * step_seconds)).isoformat().replace("+00:00", "Z")
        if step < total_poses:
            zoom = 12.5 + (drift_zoom if step == total_poses - 1 else 0.0)
            lines.append(
                f"{ts} INFO map view frame={step} zoom={zoom:.6f} "
                "center_latitude=47.260200 center_longitude=11.344000 pitch=55.000000"
            )
        pool_rev = 4 + (1 if churn_pool and step == count - 1 else 0)
        sym_rev = 2 + (1 if churn_symbol and step == count - 1 else 0)
        lines.append(
            f"{ts} INFO resident tiles tiles=24 pending={pending} settled={settled} "
            f"pool_revision={pool_rev} symbol_revision={sym_rev} finest_dem_zoom={dem_zoom}"
        )
    if extra_line is not None:
        lines.append(extra_line)
    return "\n".join(lines) + "\n"


class CheckImmersiveStabilityTests(unittest.TestCase):
    def test_passing_stationary_session_and_restart_isolation(self) -> None:
        log_text = build_log(earlier_failed_session=True)
        result = MODULE.check_log(log_text, seconds=30.0, minimum_dem_zoom=11)
        self.assertIn("PASS: 11 reports over 30.0s", result)
        self.assertIn("DEM zoom 12", result)

    def test_fails_without_residency_reports(self) -> None:
        with self.assertRaisesRegex(ValueError, "no renderer residency reports"):
            MODULE.check_log("2026-03-01T09:00:00Z INFO creating the map width=1024\n")

    def test_fails_with_short_observation_window(self) -> None:
        with self.assertRaisesRegex(ValueError, "insufficient stationary observation time"):
            MODULE.check_log(build_log(count=8, step_seconds=3.0), seconds=30.0)

    def test_fails_with_insufficient_pose_observations(self) -> None:
        with self.assertRaisesRegex(ValueError, "insufficient camera pose observations"):
            MODULE.check_log(build_log(pose_count=1))

    def test_fails_when_camera_moves(self) -> None:
        with self.assertRaisesRegex(ValueError, "camera zoom moved"):
            MODULE.check_log(build_log(drift_zoom=0.01))

    def test_fails_on_allocation_revision_churn(self) -> None:
        with self.assertRaisesRegex(ValueError, "pool_revision changed"):
            MODULE.check_log(build_log(churn_pool=True))
        with self.assertRaisesRegex(ValueError, "symbol_revision changed"):
            MODULE.check_log(build_log(churn_symbol=True))

    def test_fails_when_loading_not_settled_or_dem_too_coarse(self) -> None:
        with self.assertRaisesRegex(ValueError, "has not finished loading"):
            MODULE.check_log(build_log(pending="2"))
        with self.assertRaisesRegex(ValueError, "detailed terrain did not load"):
            MODULE.check_log(build_log(dem_zoom=9), minimum_dem_zoom=11)

    def test_fails_when_renderer_errors_appear(self) -> None:
        with self.assertRaisesRegex(ValueError, "session contains renderer failures"):
            MODULE.check_log(build_log(extra_line="2026-03-01T09:00:35Z WARN stereo frame failed"))

    def test_cli_file_and_self_test_modes(self) -> None:
        self_test = subprocess.run(
            [sys.executable, str(SCRIPT_PATH), "--self-test"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(self_test.returncode, 0, self_test.stderr)
        self.assertIn("PASS:", self_test.stdout)

        with tempfile.TemporaryDirectory() as tmp:
            good = Path(tmp) / "good.log"
            bad = Path(tmp) / "bad.log"
            good.write_text(build_log())
            bad.write_text(build_log(churn_pool=True))

            ok_proc = subprocess.run(
                [sys.executable, str(SCRIPT_PATH), str(good)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(ok_proc.returncode, 0, ok_proc.stderr)
            self.assertIn("PASS:", ok_proc.stdout)

            fail_proc = subprocess.run(
                [sys.executable, str(SCRIPT_PATH), str(bad)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(fail_proc.returncode, 0)
            self.assertIn("FAIL: pool_revision changed", fail_proc.stderr)


if __name__ == "__main__":
    unittest.main()
