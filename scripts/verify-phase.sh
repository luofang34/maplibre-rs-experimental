#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

exec python3 - "$ROOT_DIR" "$@" <<'PY'
import argparse
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def parse_args(argv):
    p = argparse.ArgumentParser(
        prog="verify-phase.sh",
        description="Parallel phase verification gate runner with fail-fast and bounded repair retries.",
    )
    p.add_argument("--base", default="origin/main", help="Git base ref for diff-based structural checks")
    p.add_argument("--phase", default="default", help="Phase name used for retry state tracking")
    p.add_argument("--max-iterations", type=int, default=3, help="Max failed iterations before escalating (default: 3)")
    p.add_argument(
        "--state-file",
        default=os.path.join(tempfile.gettempdir(), "maplibre-verify-phase-state.json"),
        help="Path to retry counter JSON state file",
    )
    p.add_argument("--reset", action="store_true", help="Reset retry counter for (headSha, phase) before running")
    p.add_argument("--repair-cmd", default="", help="Optional command invoked with <json-path> between failed iterations")
    p.add_argument("--fail-fast", dest="fail_fast", action="store_true", default=True, help="Cancel slower running gates on first failure (default)")
    p.add_argument("--no-fail-fast", dest="fail_fast", action="store_false", help="Wait for all gates even after a failure")
    p.add_argument("--include-actions", action="store_true", help="Also run network-dependent scripts/check-actions.mjs")
    p.add_argument("--json-out", default="", help="Write compact JSON summary to this path as well as stdout")
    p.add_argument("--only", default="", help="Comma-separated subset of gate names to run")
    p.add_argument("--skip", default="", help="Comma-separated gate names to skip")
    p.add_argument("--gates-file", default="", help=argparse.SUPPRESS)
    return p.parse_args(argv)


def git_head_sha(root: Path) -> str:
    try:
        out = subprocess.check_output(
            ["git", "--no-optional-locks", "-C", str(root), "rev-parse", "--short=12", "HEAD"],
            stderr=subprocess.DEVNULL,
            text=True,
        )
        return out.strip() or "unknown"
    except Exception:
        return "unknown"


def build_default_gates(root: Path, base_ref: str, include_actions: bool):
    wasm_target = os.environ.get("VERIFY_WASM_TARGET_DIR") or str(root / "target" / "verify-wasm")
    gates = [
        {
            "name": "fmt",
            "cmd": "RUSTUP_TOOLCHAIN=nightly cargo fmt -- --check",
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "file-length",
            "cmd": f'node scripts/check-file-length.mjs "{base_ref}"',
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "panics",
            "cmd": "node scripts/check-panics.mjs",
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "module-layout",
            "cmd": f'python3 scripts/check-module-layout.py --base "{base_ref}"',
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "wasm-deps",
            "cmd": "python3 scripts/check-wasm-deps.py",
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "release-version",
            "cmd": "python3 scripts/check-release-version.py",
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "immersive-stability",
            "cmd": "python3 scripts/check-immersive-stability.py --self-test",
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "web-lib",
            "cmd": "cd web/lib && npx tsc --noEmit && node --test test/*.test.mjs",
            "cwd": str(root),
            "env": {},
        },
        {
            "name": "wasm32-clippy",
            "cmd": (
                "cargo clippy --target wasm32-unknown-unknown -p web -p web-tile-worker --all-targets -- -D warnings && "
                'cargo clippy --target wasm32-unknown-unknown -p web --lib --no-default-features --features "renderer,web-webgl" -- -D warnings'
            ),
            "cwd": str(root),
            "env": {"CARGO_TARGET_DIR": wasm_target},
        },
        {
            "name": "native-clippy-test",
            "cmd": (
                "cargo clippy --workspace --exclude web --exclude web-tile-worker --exclude maplibre-android --all-targets -- -D warnings && "
                "cargo test --workspace --exclude web --exclude web-tile-worker --exclude maplibre-android --lib --tests"
            ),
            "cwd": str(root),
            "env": {},
        },
    ]
    if include_actions:
        gates.append(
            {
                "name": "actions",
                "cmd": "node scripts/check-actions.mjs",
                "cwd": str(root),
                "env": {},
            }
        )
    return gates


def load_state(path: Path) -> dict:
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def save_state(path: Path, state: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(f".tmp.{os.getpid()}")
    tmp.write_text(json.dumps(state, separators=(",", ":")), encoding="utf-8")
    tmp.replace(path)


def extract_error_tail(log_path: Path, max_lines: int = 25, max_chars: int = 2000) -> str:
    try:
        text = log_path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    lines = text.strip().splitlines()
    if not lines:
        return ""
    tail = "\n".join(lines[-max_lines:])
    if len(tail) > max_chars:
        tail = tail[-max_chars:]
    return tail


def extract_metrics(results: list, log_dir: Path) -> dict:
    metrics = {}
    for g in results:
        log_path = Path(g["logFile"])
        if not log_path.is_file():
            continue
        text = log_path.read_text(encoding="utf-8", errors="replace")
        if g["name"] == "panics":
            m = re.search(r"\((\d+)\s*<=\s*(\d+)\s+total non-test panic sites\)", text)
            if not m:
                m = re.search(r"(\d+)\s+total non-test panic sites.*limit\s+(\d+)", text)
            if m:
                metrics["panicSites"] = int(m.group(1))
                metrics["panicLimit"] = int(m.group(2))
        elif g["name"] == "file-length":
            m = re.search(r"Checked\s+(\d+)\s+changed files against the\s+(\d+)-line limit", text)
            if m:
                metrics["checkedFiles"] = int(m.group(1))
                metrics["lineLimit"] = int(m.group(2))
        elif g["name"] == "web-lib":
            mp = re.search(r"(?:ℹ|#)\s*pass\s+(\d+)", text)
            mf = re.search(r"(?:ℹ|#)\s*fail\s+(\d+)", text)
            if mp:
                metrics["webLibPass"] = int(mp.group(1))
            if mf:
                metrics["webLibFail"] = int(mf.group(1))
    return metrics


def terminate_group(proc: subprocess.Popen) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except OSError:
        pass
    deadline = time.monotonic() + 1.0
    while proc.poll() is None and time.monotonic() < deadline:
        time.sleep(0.05)
    if proc.poll() is None:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            pass
    try:
        proc.wait(timeout=1.0)
    except Exception:
        pass


def run_parallel_gates(gates: list, log_dir: Path, fail_fast: bool) -> tuple[list, int]:
    log_dir.mkdir(parents=True, exist_ok=True)
    t_all = time.monotonic()
    running = {}
    order = []

    for gate in gates:
        name = gate["name"]
        log_path = log_dir / f"{name}.log"
        fh = open(log_path, "wb")
        env = os.environ.copy()
        for k, v in (gate.get("env") or {}).items():
            env[str(k)] = str(v)
        cwd = gate.get("cwd") or "."
        proc = subprocess.Popen(
            ["bash", "-lc", gate["cmd"]],
            cwd=cwd,
            env=env,
            stdout=fh,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
        )
        rec = {
            "name": name,
            "proc": proc,
            "fh": fh,
            "logFile": str(log_path),
            "start": time.monotonic(),
            "end": None,
            "exitCode": None,
            "status": "running",
        }
        running[name] = rec
        order.append(name)

    first_fail_at = None
    try:
        while any(r["status"] == "running" for r in running.values()):
            for r in running.values():
                if r["status"] != "running":
                    continue
                rc = r["proc"].poll()
                if rc is not None:
                    r["end"] = time.monotonic()
                    r["exitCode"] = rc
                    r["status"] = "pass" if rc == 0 else "fail"
                    r["fh"].close()
                    if rc != 0 and first_fail_at is None:
                        first_fail_at = time.monotonic()

            if fail_fast and first_fail_at is not None and (time.monotonic() - first_fail_at) >= 0.5:
                for r in running.values():
                    if r["status"] == "running":
                        terminate_group(r["proc"])
                        r["end"] = time.monotonic()
                        r["exitCode"] = -1
                        r["status"] = "cancelled"
                        try:
                            r["fh"].close()
                        except Exception:
                            pass
                break
            if any(r["status"] == "running" for r in running.values()):
                time.sleep(0.05)
    finally:
        for r in running.values():
            if r["proc"].poll() is None:
                terminate_group(r["proc"])
                r["status"] = "cancelled"
                r["exitCode"] = -1
                r["end"] = time.monotonic()
            try:
                r["fh"].close()
            except Exception:
                pass

    results = []
    for name in order:
        r = running[name]
        dur_ms = int(round(((r["end"] or time.monotonic()) - r["start"]) * 1000.0))
        tail = extract_error_tail(Path(r["logFile"])) if r["status"] != "pass" else ""
        results.append(
            {
                "name": name,
                "status": r["status"],
                "exitCode": r["exitCode"],
                "durationMs": dur_ms,
                "logFile": r["logFile"],
                "errorTail": tail,
            }
        )
    total_ms = int(round((time.monotonic() - t_all) * 1000.0))
    return results, total_ms


def main() -> int:
    root = Path(sys.argv[1]).resolve()
    args = parse_args(sys.argv[2:])
    max_iters = max(1, args.max_iterations)

    if args.gates_file:
        all_gates = json.loads(Path(args.gates_file).read_text(encoding="utf-8"))
    else:
        all_gates = build_default_gates(root, args.base, args.include_actions)

    only_set = {x.strip() for x in args.only.split(",") if x.strip()}
    skip_set = {x.strip() for x in args.skip.split(",") if x.strip()}
    gates = [
        g for g in all_gates
        if (not only_set or g["name"] in only_set) and g["name"] not in skip_set
    ]
    if not gates:
        print(json.dumps({"schemaVersion": 1, "status": "fail", "error": "no gates selected"}))
        return 1

    head_sha = git_head_sha(root)
    state_path = Path(args.state_file)
    state_key = f"{head_sha}:{args.phase}:{hashlib.sha1(str(root).encode()).hexdigest()[:8]}"
    state = load_state(state_path)
    if args.reset:
        state.pop(state_key, None)
        save_state(state_path, state)

    log_base = Path(tempfile.gettempdir()) / f"maplibre-verify-phase-{args.phase}-{os.getpid()}"

    while True:
        state = load_state(state_path)
        prev_failures = int(state.get(state_key, 0))
        if prev_failures >= max_iters:
            payload = {
                "schemaVersion": 1,
                "status": "escalate",
                "escalate": True,
                "iteration": prev_failures,
                "maxIterations": max_iters,
                "remainingRepairs": 0,
                "baseRef": args.base,
                "headSha": head_sha,
                "phase": args.phase,
                "durationMs": 0,
                "failedGates": [],
                "cancelledGates": [],
                "parsedMetrics": {},
                "gates": [],
                "message": f"Reached max iterations ({max_iters}) without passing; escalate before editing further.",
            }
            out_json = json.dumps(payload, separators=(",", ":"))
            if args.json_out:
                Path(args.json_out).parent.mkdir(parents=True, exist_ok=True)
                Path(args.json_out).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
            print(out_json)
            return 2

        iteration = prev_failures + 1
        gate_results, duration_ms = run_parallel_gates(gates, log_base / f"iter-{iteration}", args.fail_fast)
        failed = [g["name"] for g in gate_results if g["status"] == "fail"]
        cancelled = [g["name"] for g in gate_results if g["status"] == "cancelled"]
        passed = len(failed) == 0 and len(cancelled) == 0
        metrics = extract_metrics(gate_results, log_base / f"iter-{iteration}")

        if passed:
            state.pop(state_key, None)
            save_state(state_path, state)
            payload = {
                "schemaVersion": 1,
                "status": "pass",
                "escalate": False,
                "iteration": iteration,
                "maxIterations": max_iters,
                "remainingRepairs": max_iters,
                "baseRef": args.base,
                "headSha": head_sha,
                "phase": args.phase,
                "durationMs": duration_ms,
                "failedGates": [],
                "cancelledGates": [],
                "parsedMetrics": metrics,
                "gates": gate_results,
            }
            out_json = json.dumps(payload, separators=(",", ":"))
            if args.json_out:
                Path(args.json_out).parent.mkdir(parents=True, exist_ok=True)
                Path(args.json_out).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
            print(out_json)
            return 0

        state[state_key] = iteration
        save_state(state_path, state)
        remaining = max(0, max_iters - iteration)
        escalate = iteration >= max_iters
        payload = {
            "schemaVersion": 1,
            "status": "escalate" if escalate else "fail",
            "escalate": escalate,
            "iteration": iteration,
            "maxIterations": max_iters,
            "remainingRepairs": remaining,
            "baseRef": args.base,
            "headSha": head_sha,
            "phase": args.phase,
            "durationMs": duration_ms,
            "failedGates": failed,
            "cancelledGates": cancelled,
            "parsedMetrics": metrics,
            "gates": gate_results,
        }
        out_path = Path(args.json_out) if args.json_out else (log_base / f"iter-{iteration}-summary.json")
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

        if not escalate and args.repair_cmd:
            rc = subprocess.run(["bash", "-lc", f'{args.repair_cmd} "{out_path}"'], cwd=str(root)).returncode
            if rc == 0:
                continue

        print(json.dumps(payload, separators=(",", ":")))
        return 2 if escalate else 1


if __name__ == "__main__":
    raise SystemExit(main())
PY
