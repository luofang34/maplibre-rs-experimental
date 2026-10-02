import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';

const origin = new URL(process.argv[2] ?? 'http://localhost:4174');
const output = new URL('../dist/', import.meta.url);
const assets = await readdir(new URL('assets/', output));
const worker = assets.find(name => /^maplibre-gl-worker-.*\.js$/.test(name));
assert.ok(worker, 'The production build must include its MapLibre worker.');
const entries = assets.filter(name => /^index-.*\.js$/.test(name));
assert.ok(entries.length > 0);
const entry = (await Promise.all(entries.map(name => readFile(new URL(`assets/${name}`, output), 'utf8')))).join('\n');
assert.ok(entry.includes(`/assets/${worker}`), 'The map must reference the emitted worker URL.');

// Comparing responses also catches SPA fallbacks returning HTML for missing workers/data.
const paths = ['index.html', ...assets.map(name => `assets/${name}`),
  'data/terrain-style.json', 'data/innsbruck-approach.json', 'data/mach-loop.json'];
for (const path of paths) {
  const response = await fetch(new URL(path, origin), { signal: AbortSignal.timeout(10000) });
  assert.equal(response.status, 200, `${path} must be served`);
  if (path.endsWith('.js')) assert.match(response.headers.get('content-type') ?? '', /javascript/);
  assert.deepEqual(Buffer.from(await response.arrayBuffer()), await readFile(new URL(path, output)), `${path} must match the built asset`);
}
console.info(`Verified ${paths.length} deployed files, including the executable MapLibre worker, at ${origin.origin}`);
