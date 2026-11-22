import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import test from 'node:test';
import { deflateSync } from 'node:zlib';

import {
  analyzeImageContent,
  compareRgbaBuffers,
  decodePng,
  encodeRgbaPng,
  formatDiffReport,
  resolveBundleAssetPath,
} from './browser-visual-diff-lib.mjs';

function buildFilteredPng(width, height, rgba, filterModes) {
  // Reuse encodeRgbaPng's header/CRC structure by swapping IDAT scanlines via custom PNG builder.
  const base = encodeRgbaPng(width, height, rgba);
  const sig = base.subarray(0, 8);
  const ihdr = base.subarray(8, 33);
  const iend = base.subarray(base.length - 12);

  const stride = width * 4;
  const raw = Buffer.alloc(height * (stride + 1));
  for (let y = 0; y < height; y++) {
    const f = filterModes[y % filterModes.length];
    const rowOut = y * (stride + 1);
    raw[rowOut] = f;
    for (let x = 0; x < stride; x++) {
      const cur = rgba[y * stride + x];
      const left = x >= 4 ? rgba[y * stride + x - 4] : 0;
      const up = y > 0 ? rgba[(y - 1) * stride + x] : 0;
      const upLeft = y > 0 && x >= 4 ? rgba[(y - 1) * stride + x - 4] : 0;
      let val = cur;
      if (f === 1) val = (cur - left) & 0xff;
      else if (f === 2) val = (cur - up) & 0xff;
      else if (f === 3) val = (cur - ((left + up) >>> 1)) & 0xff;
      else if (f === 4) {
        const p = left + up - upLeft;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - upLeft);
        const pr = pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
        val = (cur - pr) & 0xff;
      }
      raw[rowOut + 1 + x] = val;
    }
  }
  const idatPayload = deflateSync(raw);
  const idat = Buffer.alloc(12 + idatPayload.length);
  idat.writeUInt32BE(idatPayload.length, 0);
  idat.write('IDAT', 4, 4, 'ascii');
  idatPayload.copy(idat, 8);
  idat.writeUInt32BE(0, 8 + idatPayload.length); // decodePng doesn't enforce CRC mismatch if not needed
  return Buffer.concat([sig, ihdr, idat, iend]);
}

test('encodeRgbaPng and decodePng round-trip RGBA images across all 5 PNG filter modes', () => {
  const width = 5;
  const height = 5;
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    rgba[i * 4] = (i * 37) & 0xff;
    rgba[i * 4 + 1] = (i * 73 + 11) & 0xff;
    rgba[i * 4 + 2] = (i * 19 + 90) & 0xff;
    rgba[i * 4 + 3] = 255;
  }

  const encoded = encodeRgbaPng(width, height, rgba);
  const decoded = decodePng(encoded);
  assert.equal(decoded.width, width);
  assert.equal(decoded.height, height);
  assert.deepEqual(decoded.rgba, rgba);

  const filtered = buildFilteredPng(width, height, rgba, [0, 1, 2, 3, 4]);
  const decodedFiltered = decodePng(filtered);
  assert.deepEqual(decodedFiltered.rgba, rgba);
});

test('compareRgbaBuffers and analyzeImageContent detect pixel diffs and blank frames', () => {
  const w = 4;
  const h = 4;
  const a = new Uint8Array(w * h * 4);
  const b = new Uint8Array(w * h * 4);
  for (let i = 0; i < w * h; i++) {
    a[i * 4] = i * 15;
    a[i * 4 + 1] = 40;
    a[i * 4 + 2] = 200;
    a[i * 4 + 3] = 255;
    b.set(a.subarray(i * 4, i * 4 + 4), i * 4);
  }
  b[4] += 20; // differ by 20 (> threshold 8) at pixel 1

  const cmp = compareRgbaBuffers(a, b, w, h, { channelThreshold: 8 });
  assert.equal(cmp.diffPixels, 1);
  assert.equal(cmp.maxChannelDiff, 20);
  assert.equal(cmp.diffRgba[4], 255);

  const rich = analyzeImageContent(a, w, h);
  assert.equal(rich.isUniform, false);
  assert.ok(rich.distinctSampleColors > 4);

  const blank = new Uint8Array(w * h * 4).fill(255);
  assert.equal(analyzeImageContent(blank, w, h).isUniform, true);
});

test('resolveBundleAssetPath strips ?hash queries and blocks directory traversal', () => {
  const dir = '/tmp/bundle';
  assert.equal(
    resolveBundleAssetPath(dir, '/assets/maplibre_bg.wasm?hash=abc123#frag'),
    resolve(dir, 'assets', 'maplibre_bg.wasm'),
  );
  assert.equal(resolveBundleAssetPath(dir, '/../../etc/passwd'), null);
  assert.equal(resolveBundleAssetPath(dir, '/%2e%2e/secret'), null);
});

test('formatDiffReport renders Markdown tables for captures and comparisons', () => {
  const md = formatDiffReport({
    generatedAt: '2026-11-21T00:00:00Z',
    maxDiffRatio: 0.02,
    channelThreshold: 8,
    captures: [
      {
        browser: 'chromium',
        scene: 'globe-world',
        backend: 'webgpu',
        activeBackend: 'webgpu',
        content: { maxChannelSpan: 180, distinctSampleColors: 42 },
        status: 'ok',
      },
    ],
    comparisons: [
      {
        label: 'chromium:webgpu vs chromium:webgl',
        scene: 'globe-world',
        diffPixels: 10,
        totalPixels: 1000,
        diffRatio: 0.01,
        maxChannelDiff: 12,
        meanAbsDiff: 0.25,
        ok: true,
      },
    ],
  });
  assert.match(md, /\| chromium \| globe-world \| webgpu \| webgpu \| 42 \| 180 \| ok \|/);
  assert.match(md, /\| chromium:webgpu vs chromium:webgl \| globe-world \| 10\/1000 \| 1\.000% \| 12 \| 0\.25 \| PASS \|/);
});
