#!/usr/bin/env python3
"""Snapshot the complete Indicate dependency closure from an explicit source checkout.

Usage:
  sync-indicate.py <indicate-checkout> [--allow-unreproducible]
  sync-indicate.py --check [--require-clean]

A snapshot is reproducible only from a clean checkout whose tree matches the
tree of a remote branch. The tree, not the commit, is compared, so a clean
GitButler workspace commit qualifies once its content is published. Sync
refuses other checkouts unless --allow-unreproducible is given, and --check
reports such a snapshot. --require-clean makes that report fatal.
"""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

project = Path(__file__).resolve().parent.parent
overlay = project / 'IndicateOverlay'
destination = overlay / 'vendor/indicate'
USAGE = 'usage: sync-indicate.py <indicate-checkout> [--allow-unreproducible] | --check [--require-clean]'
arguments = sys.argv[1:]
if arguments and arguments[0] == '--check':
    if arguments[1:] not in ([], ['--require-clean']):
        raise SystemExit(USAGE)
    record = json.loads((destination / 'SOURCE.json').read_text())
    actual = {str(p.relative_to(destination)): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in sorted(destination.rglob('*')) if p.is_file() and p.name != 'SOURCE.json'}
    digest = hashlib.sha256(json.dumps(actual, sort_keys=True).encode()).hexdigest()
    if actual != record['files'] or digest != record['snapshot_sha256']:
        raise SystemExit('Indicate snapshot changed. Synchronize the source again.')
    if record.get('includes_working_tree_changes', True) or not record.get('base_tree_published', False):
        message = (f"Indicate snapshot of tree {record.get('base_tree', 'unknown')} is not reproducible "
                   'from a published Indicate branch. Publish the source and synchronize again.')
        if '--require-clean' in arguments:
            raise SystemExit(message)
        print(f'WARNING: {message}', file=sys.stderr)
    print(f"Verified Indicate snapshot: {record['snapshot_sha256']}")
    raise SystemExit(0)
if not arguments or arguments[0].startswith('--') or arguments[1:] not in ([], ['--allow-unreproducible']):
    raise SystemExit(USAGE)
source = Path(arguments[0]).resolve()
dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=source))
tree = subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], cwd=source, text=True).strip()
remote_trees = subprocess.check_output(['git', 'for-each-ref', 'refs/remotes', '--format=%(tree)'],
                                       cwd=source, text=True).split()
published = tree in remote_trees
if (dirty or not published) and '--allow-unreproducible' not in arguments:
    reason = 'has uncommitted changes' if dirty else 'HEAD tree matches no remote branch'
    raise SystemExit(f'{source} {reason}. Sync from a published commit, or pass --allow-unreproducible.')
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--no-deps',
    '--format-version', '1', '--manifest-path', str(source / 'Cargo.toml')], cwd=source))
packages = {p['name']: p for p in metadata['packages']}
subprocess.run(['cargo', 'test', '--locked', '-p', 'hwd-bench', '-p', 'indicate-instrument-hmd'], cwd=source, check=True)
selected, pending = set(), ['indicate-instrument-hmd']
while pending:
    name = pending.pop()
    if name in selected:
        continue
    selected.add(name)
    pending.extend(d['name'] for d in packages[name]['dependencies'] if d['name'] in packages)
if destination.exists():
    shutil.rmtree(destination)
destination.mkdir(parents=True)
members = []
for name in sorted(selected):
    directory = Path(packages[name]['manifest_path']).parent
    relative = directory.relative_to(source)
    members.append(str(relative))
    shutil.copytree(directory, destination / relative)
for relative in ['docs/instruments/display-reason-registry.md', 'docs/instruments/scene-layer-protocol.md',
                 'docs/instruments/hwd-presentation-policy.md', 'clippy.toml']:
    path = source / relative
    if path.exists():
        (destination / relative).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, destination / relative)
manifest = (source / 'Cargo.toml').read_text()
manifest = re.sub(r'members = \[.*?\]', 'members = ' + json.dumps(members), manifest, count=1, flags=re.S)
(destination / 'Cargo.toml').write_text(manifest)
files = {str(p.relative_to(destination)): hashlib.sha256(p.read_bytes()).hexdigest()
         for p in sorted(destination.rglob('*')) if p.is_file()}
record = {
    'repository': 'https://github.com/luofang34/Indicate',
    'base_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip(),
    'includes_working_tree_changes': dirty,
    'base_tree': tree,
    'base_tree_published': published,
    'snapshot_sha256': hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest(),
    'files': files,
}
(destination / 'SOURCE.json').write_text(json.dumps(record, indent=2) + '\n')
print(f"Snapshotted {len(selected)} packages, {len(files)} files: {record['snapshot_sha256']}")
