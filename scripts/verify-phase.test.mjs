import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, '..');
const VERIFY_SH = join(SCRIPT_DIR, 'verify-phase.sh');

function runVerify(extraArgs) {
  const res = spawnSync('bash', [VERIFY_SH, ...extraArgs], {
    cwd: REPO_ROOT,
    encoding: 'utf8',
  });
  return {
    status: res.status,
    stdout: res.stdout,
    stderr: res.stderr,
    json: JSON.parse(res.stdout.trim()),
  };
}

test('runs fast structural gates in parallel and emits compact JSON', () => {
  const work = mkdtempSync(join(tmpdir(), 'verify-phase-struct-'));
  const stateFile = join(work, 'state.json');
  const jsonOut = join(work, 'summary.json');
  try {
    const out = runVerify([
      '--phase', 'struct-test',
      '--state-file', stateFile,
      '--json-out', jsonOut,
      '--reset',
      '--only', 'fmt,evidence-gate,release-version,immersive-stability',
    ]);
    assert.equal(out.status, 0, out.stderr);
    assert.equal(out.json.status, 'pass');
    assert.equal(out.json.escalate, false);
    assert.equal(out.json.iteration, 1);
    assert.deepEqual(out.json.failedGates, []);
    assert.equal(out.json.gates.length, 4);
    assert.equal(out.json.parsedMetrics.evidenceGatePassed, true);
    assert.ok(out.json.parsedMetrics.evidenceGateChecks > 0);
    assert.ok(readFileSync(jsonOut, 'utf8').includes('"status": "pass"'));
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test('caps repair attempts at 3 iterations before escalating and resets on pass', () => {
  const work = mkdtempSync(join(tmpdir(), 'verify-phase-escalate-'));
  const stateFile = join(work, 'state.json');
  const gatesFile = join(work, 'gates.json');
  const marker = join(work, 'fix.flag');
  try {
    writeFileSync(
      gatesFile,
      JSON.stringify([
        { name: 'fast-pass', cmd: 'echo ok', cwd: REPO_ROOT, env: {} },
        { name: 'flaky', cmd: `test -f "${marker}"`, cwd: REPO_ROOT, env: {} },
      ]),
    );

    const r1 = runVerify(['--phase', 'esc', '--max-iterations', '3', '--state-file', stateFile, '--gates-file', gatesFile]);
    assert.equal(r1.status, 1);
    assert.equal(r1.json.status, 'fail');
    assert.equal(r1.json.escalate, false);
    assert.equal(r1.json.iteration, 1);
    assert.equal(r1.json.remainingRepairs, 2);

    const r2 = runVerify(['--phase', 'esc', '--max-iterations', '3', '--state-file', stateFile, '--gates-file', gatesFile]);
    assert.equal(r2.status, 1);
    assert.equal(r2.json.status, 'fail');
    assert.equal(r2.json.escalate, false);
    assert.equal(r2.json.iteration, 2);
    assert.equal(r2.json.remainingRepairs, 1);

    const r3 = runVerify(['--phase', 'esc', '--max-iterations', '3', '--state-file', stateFile, '--gates-file', gatesFile]);
    assert.equal(r3.status, 2);
    assert.equal(r3.json.status, 'escalate');
    assert.equal(r3.json.escalate, true);
    assert.equal(r3.json.iteration, 3);
    assert.equal(r3.json.remainingRepairs, 0);

    // Subsequent call stays escalated until --reset
    const r4 = runVerify(['--phase', 'esc', '--max-iterations', '3', '--state-file', stateFile, '--gates-file', gatesFile]);
    assert.equal(r4.status, 2);
    assert.equal(r4.json.escalate, true);

    // Passing after repair clears retry counter
    writeFileSync(marker, 'ok');
    const rReset = runVerify(['--phase', 'esc', '--max-iterations', '3', '--state-file', stateFile, '--gates-file', gatesFile, '--reset']);
    assert.equal(rReset.status, 0);
    assert.equal(rReset.json.status, 'pass');
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test('fail-fast cancels slower running siblings on gate failure', () => {
  const work = mkdtempSync(join(tmpdir(), 'verify-phase-ff-'));
  const stateFile = join(work, 'state.json');
  const gatesFile = join(work, 'gates.json');
  try {
    writeFileSync(
      gatesFile,
      JSON.stringify([
        { name: 'fail-immediately', cmd: 'echo boom >&2; exit 3', cwd: REPO_ROOT, env: {} },
        { name: 'slow-sibling', cmd: 'sleep 15', cwd: REPO_ROOT, env: {} },
      ]),
    );
    const t0 = Date.now();
    const res = runVerify(['--phase', 'ff', '--state-file', stateFile, '--gates-file', gatesFile]);
    const elapsedMs = Date.now() - t0;
    assert.equal(res.status, 1);
    assert.deepEqual(res.json.failedGates, ['fail-immediately']);
    assert.deepEqual(res.json.cancelledGates, ['slow-sibling']);
    assert.ok(elapsedMs < 5000, `fail-fast took too long: ${elapsedMs}ms`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});
