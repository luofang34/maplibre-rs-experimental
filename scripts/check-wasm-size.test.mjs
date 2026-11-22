import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { evaluateBudget, measureDistDir, normalizeAssetName } from './check-wasm-size.mjs';

test('normalizeAssetName strips esbuild content hashes before .wasm', () => {
  assert.equal(normalizeAssetName('assets/maplibre_bg-A1B2C3D4.wasm'), 'assets/maplibre_bg.wasm');
  assert.equal(normalizeAssetName('assets/maplibre_bg.wasm'), 'assets/maplibre_bg.wasm');
  assert.equal(
    normalizeAssetName('assets/maplibre_tile_worker_bg-9z8y7x6w5v.wasm'),
    'assets/maplibre_tile_worker_bg.wasm',
  );
  assert.equal(normalizeAssetName('module.js'), 'module.js');
});

test('measureDistDir computes raw and gzip bytes and rejects missing .wasm', () => {
  const dir = mkdtempSync(join(tmpdir(), 'wasm-size-test-'));
  try {
    mkdirSync(join(dir, 'assets'), { recursive: true });
    writeFileSync(join(dir, 'module.js'), 'console.log("hello world");\n');
    const wasmPayload = Buffer.alloc(4096, 0x2a);
    wasmPayload[0] = 0x00;
    wasmPayload[1] = 0x61;
    wasmPayload[2] = 0x73;
    wasmPayload[3] = 0x6d;
    writeFileSync(join(dir, 'assets', 'maplibre_bg-AbCd1234.wasm'), wasmPayload);
    writeFileSync(join(dir, 'assets', 'ignored.txt'), 'not counted');

    const m = measureDistDir(dir);
    assert.deepEqual(Object.keys(m.artifacts), ['assets/maplibre_bg.wasm', 'module.js']);
    assert.equal(m.artifacts['assets/maplibre_bg.wasm'].rawBytes, 4096);
    assert.ok(m.artifacts['assets/maplibre_bg.wasm'].gzipBytes > 0);
    assert.equal(m.totalRawBytes, 4096 + m.artifacts['module.js'].rawBytes);

    rmSync(join(dir, 'assets', 'maplibre_bg-AbCd1234.wasm'));
    assert.throws(() => measureDistDir(dir), /No compiled \.wasm artifact/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('evaluateBudget passes within +3% tolerance and fails on bloat or unknown variant', () => {
  const budget = {
    tolerance: 0.03,
    variants: {
      webgpu: {
        artifacts: {
          'assets/maplibre_bg.wasm': { rawBytes: 1000, gzipBytes: 500 },
          'module.js': { rawBytes: 200, gzipBytes: 100 },
        },
        totalRawBytes: 1200,
        totalGzipBytes: 600,
      },
    },
  };

  const within = evaluateBudget({
    variant: 'webgpu',
    budget,
    measurement: {
      artifacts: {
        'assets/maplibre_bg.wasm': { rawBytes: 1030, gzipBytes: 515 },
        'module.js': { rawBytes: 206, gzipBytes: 103 },
      },
      totalRawBytes: 1236,
      totalGzipBytes: 618,
    },
  });
  assert.equal(within.ok, true);
  assert.equal(within.errors.length, 0);

  const over = evaluateBudget({
    variant: 'webgpu',
    budget,
    measurement: {
      artifacts: {
        'assets/maplibre_bg.wasm': { rawBytes: 1040, gzipBytes: 515 },
        'module.js': { rawBytes: 200, gzipBytes: 100 },
      },
      totalRawBytes: 1240,
      totalGzipBytes: 615,
    },
  });
  assert.equal(over.ok, false);
  assert.ok(over.errors.some((e) => e.includes('maplibre_bg.wasm raw size')));
  assert.ok(over.errors.some((e) => e.includes('total raw size')));

  assert.throws(
    () =>
      evaluateBudget({
        variant: 'invalid-variant',
        budget,
        measurement: { artifacts: {}, totalRawBytes: 0, totalGzipBytes: 0 },
      }),
    /Unknown variant/,
  );
});
