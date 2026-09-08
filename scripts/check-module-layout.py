#!/usr/bin/env python3
"""Reject additions or edits of mod.rs while allowing removal of legacy modules."""
import argparse
from pathlib import Path
import subprocess
import sys


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
    parser.add_argument("--base", default="HEAD", help="Comparison commit or PR base")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    failures = rejected(changed_paths(args.root, args.base))
    if failures:
        sys.stderr.write("Use foo.rs with foo/ for changed modules:\n" + "\n".join(failures) + "\n")
        return 1
    sys.stdout.write("Changed module layout passed.\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
