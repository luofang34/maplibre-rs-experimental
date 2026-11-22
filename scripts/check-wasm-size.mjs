import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, posix, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { gzipSync } from 'node:zlib';

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(SCRIPT_DIR, '..');
export const DEFAULT_DIST_DIR = join(REPO_ROOT, 'web', 'lib', 'dist', 'esbuild-esm');
export const DEFAULT_BUDGET_PATH = join(SCRIPT_DIR, 'wasm-size-budget.json');
export const KNOWN_VARIANTS = ['webgpu', 'webgl', 'webgl-mt'];
export const DEFAULT_TOLERANCE = 0.03;
const MIN_REAL_WASM_BYTES = 1024;

export function normalizeAssetName(relPath) {
  const norm = relPath.split('\\').join('/');
  return norm.replace(/-[A-Za-z0-9_]{8,}(?=\.wasm$)/, '');
}

function collectFiles(rootDir, currentDir = rootDir, out = []) {
  for (const entry of readdirSync(currentDir, { withFileTypes: true })) {
    const abs = join(currentDir, entry.name);
    if (entry.isDirectory()) {
      collectFiles(rootDir, abs, out);
    } else if (entry.isFile()) {
      const rel = abs.slice(rootDir.length + 1).split('\\').join('/');
      out.push([posix.normalize(rel), abs]);
    }
  }
  return out;
}

export function measureDistDir(distDir) {
  if (!existsSync(distDir) || !statSync(distDir).isDirectory()) {
    throw new Error(`WASM dist directory does not exist: ${distDir}`);
  }
  const allFiles = collectFiles(distDir);
  const selected = allFiles
    .filter(([rel]) => rel === 'module.js' || rel.endsWith('.wasm'))
    .map(([rel, abs]) => [normalizeAssetName(rel), abs])
    .sort(([a], [b]) => a.localeCompare(b));

  const wasmSizes = selected
    .filter(([name]) => name.endsWith('.wasm'))
    .map(([, abs]) => statSync(abs).size);
  if (!wasmSizes.some((size) => size > MIN_REAL_WASM_BYTES)) {
    throw new Error(
      `No compiled .wasm artifact larger than ${MIN_REAL_WASM_BYTES} bytes found in ${distDir}`,
    );
  }

  const artifacts = {};
  let totalRawBytes = 0;
  let totalGzipBytes = 0;

  for (const [name, abs] of selected) {
    if (artifacts[name]) {
      throw new Error(`Duplicate normalized asset name in ${distDir}: ${name}`);
    }
    const bytes = readFileSync(abs);
    const rawBytes = bytes.byteLength;
    const gzipBytes = gzipSync(bytes, { level: 9 }).byteLength;
    artifacts[name] = { rawBytes, gzipBytes };
    totalRawBytes += rawBytes;
    totalGzipBytes += gzipBytes;
  }

  return { artifacts, totalRawBytes, totalGzipBytes };
}

function formatDeltaPct(actual, baseline) {
  if (baseline === 0) {
    return actual === 0 ? '+0.00%' : '+inf%';
  }
  const pct = ((actual - baseline) / baseline) * 100;
  const sign = pct >= 0 ? '+' : '';
  return `${sign}${pct.toFixed(2)}%`;
}

export function evaluateBudget({ variant, measurement, budget, tolerance }) {
  if (!KNOWN_VARIANTS.includes(variant)) {
    throw new Error(
      `Unknown variant "${variant}". Expected one of: ${KNOWN_VARIANTS.join(', ')}`,
    );
  }
  const baseline = budget?.variants?.[variant];
  if (!baseline) {
    throw new Error(`No baseline entry for variant "${variant}" in budget file`);
  }

  const tol = tolerance ?? budget.tolerance ?? DEFAULT_TOLERANCE;
  if (!Number.isFinite(tol) || tol < 0) {
    throw new Error(`Invalid tolerance: ${tol}`);
  }

  const ceiling = (base) => Math.ceil(base * (1 + tol));
  const errors = [];
  const rows = [];

  for (const [name, base] of Object.entries(baseline.artifacts)) {
    const cur = measurement.artifacts[name];
    if (!cur) {
      errors.push(`[${variant}] Missing budgeted artifact: ${name}`);
      continue;
    }
    const maxRaw = ceiling(base.rawBytes);
    const maxGzip = ceiling(base.gzipBytes);
    if (cur.rawBytes > maxRaw) {
      errors.push(
        `[${variant}] ${name} raw size ${cur.rawBytes} B exceeds ceiling ${maxRaw} B ` +
          `(baseline ${base.rawBytes} B, ${formatDeltaPct(cur.rawBytes, base.rawBytes)})`,
      );
    }
    if (cur.gzipBytes > maxGzip) {
      errors.push(
        `[${variant}] ${name} gzip size ${cur.gzipBytes} B exceeds ceiling ${maxGzip} B ` +
          `(baseline ${base.gzipBytes} B, ${formatDeltaPct(cur.gzipBytes, base.gzipBytes)})`,
      );
    }
    rows.push({
      name,
      rawBytes: cur.rawBytes,
      baseRawBytes: base.rawBytes,
      maxRawBytes: maxRaw,
      gzipBytes: cur.gzipBytes,
      baseGzipBytes: base.gzipBytes,
      maxGzipBytes: maxGzip,
    });
  }

  const maxTotalRaw = ceiling(baseline.totalRawBytes);
  const maxTotalGzip = ceiling(baseline.totalGzipBytes);
  if (measurement.totalRawBytes > maxTotalRaw) {
    errors.push(
      `[${variant}] total raw size ${measurement.totalRawBytes} B exceeds ceiling ${maxTotalRaw} B ` +
        `(baseline ${baseline.totalRawBytes} B, ${formatDeltaPct(measurement.totalRawBytes, baseline.totalRawBytes)})`,
    );
  }
  if (measurement.totalGzipBytes > maxTotalGzip) {
    errors.push(
      `[${variant}] total gzip size ${measurement.totalGzipBytes} B exceeds ceiling ${maxTotalGzip} B ` +
        `(baseline ${baseline.totalGzipBytes} B, ${formatDeltaPct(measurement.totalGzipBytes, baseline.totalGzipBytes)})`,
    );
  }

  const totalRow = {
    name: 'TOTAL',
    rawBytes: measurement.totalRawBytes,
    baseRawBytes: baseline.totalRawBytes,
    maxRawBytes: maxTotalRaw,
    gzipBytes: measurement.totalGzipBytes,
    baseGzipBytes: baseline.totalGzipBytes,
    maxGzipBytes: maxTotalGzip,
  };

  return { ok: errors.length === 0, tolerance: tol, rows, totalRow, errors };
}

function parseArgs(argv) {
  const opts = {
    variant: null,
    dist: DEFAULT_DIST_DIR,
    budget: DEFAULT_BUDGET_PATH,
    writeBaseline: false,
    tolerance: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--variant') {
      opts.variant = argv[++i] ?? null;
    } else if (arg === '--dist') {
      opts.dist = resolve(argv[++i] ?? '');
    } else if (arg === '--budget') {
      opts.budget = resolve(argv[++i] ?? '');
    } else if (arg === '--tolerance') {
      opts.tolerance = Number(argv[++i]);
    } else if (arg === '--write-baseline') {
      opts.writeBaseline = true;
    } else {
      throw new Error(`Unknown CLI argument: ${arg}`);
    }
  }
  if (!opts.variant) {
    throw new Error(`Missing required --variant <${KNOWN_VARIANTS.join('|')}>`);
  }
  if (!KNOWN_VARIANTS.includes(opts.variant)) {
    throw new Error(
      `Unknown variant "${opts.variant}". Expected one of: ${KNOWN_VARIANTS.join(', ')}`,
    );
  }
  return opts;
}

function printReport(variant, result) {
  const pct = `+${(result.tolerance * 100).toFixed(1)}%`;
  console.log(`WASM size report for ${variant} (ratchet tolerance ${pct}):`);
  for (const row of [...result.rows, result.totalRow]) {
    console.log(
      `  ${row.name}: raw ${row.rawBytes} B (base ${row.baseRawBytes} B, max ${row.maxRawBytes} B, ` +
        `${formatDeltaPct(row.rawBytes, row.baseRawBytes)}) | gzip ${row.gzipBytes} B ` +
        `(base ${row.baseGzipBytes} B, max ${row.maxGzipBytes} B, ${formatDeltaPct(row.gzipBytes, row.baseGzipBytes)})`,
    );
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const opts = parseArgs(process.argv.slice(2));
  const measurement = measureDistDir(opts.dist);

  if (opts.writeBaseline) {
    let budget = { tolerance: DEFAULT_TOLERANCE, variants: {} };
    if (existsSync(opts.budget)) {
      budget = JSON.parse(readFileSync(opts.budget, 'utf8'));
    }
    if (opts.tolerance !== null) {
      if (!Number.isFinite(opts.tolerance) || opts.tolerance < 0) {
        throw new Error(`Invalid tolerance: ${opts.tolerance}`);
      }
      budget.tolerance = opts.tolerance;
    }
    budget.variants ??= {};
    budget.variants[opts.variant] = measurement;
    mkdirSync(dirname(opts.budget), { recursive: true });
    writeFileSync(opts.budget, `${JSON.stringify(budget, null, 2)}\n`);
    console.log(
      `Recorded WASM baseline for ${opts.variant}: raw=${measurement.totalRawBytes} B, gzip=${measurement.totalGzipBytes} B`,
    );
  } else {
    if (!existsSync(opts.budget)) {
      throw new Error(`Missing WASM budget file: ${opts.budget}`);
    }
    const budget = JSON.parse(readFileSync(opts.budget, 'utf8'));
    const result = evaluateBudget({
      variant: opts.variant,
      measurement,
      budget,
      tolerance: opts.tolerance,
    });
    printReport(opts.variant, result);
    if (!result.ok) {
      for (const err of result.errors) {
        console.error(err);
      }
      process.exitCode = 1;
    }
  }
}
