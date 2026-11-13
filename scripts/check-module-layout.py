#!/usr/bin/env python3
"""Reject any mod.rs files in the repository."""
import argparse
from pathlib import Path
import subprocess
import sys


def tracked_and_untracked_paths(root):
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).split(b"\0")
    paths += subprocess.check_output(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=root,
    ).split(b"\0")
    return {path.decode() for path in paths if path}


def changed_paths(root, base):
    paths = subprocess.check_output(
        ["git", "diff", "--no-renames", "--name-only", "--diff-filter=AM", "-z", base, "--"],
        cwd=root,
    ).split(b"\0")
    paths += subprocess.check_output(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=root,
    ).split(b"\0")
    return {path.decode() for path in paths if path}


def rejected(paths):
    return sorted(path for path in paths if Path(path).name == "mod.rs")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default=None, help="Optional comparison commit or PR base")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    paths = tracked_and_untracked_paths(args.root)
    if args.base is not None:
        paths |= changed_paths(args.root, args.base)
    failures = rejected(paths)
    if failures:
        sys.stderr.write("Use foo.rs with foo/ instead of mod.rs:\n" + "\n".join(failures) + "\n")
        return 1
    sys.stdout.write("Module layout passed (zero mod.rs files).\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
