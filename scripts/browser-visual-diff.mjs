import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import {
  analyzeImageContent,
  compareRgbaBuffers,
  decodePng,
  encodeRgbaPng,
  formatDiffReport,
  resolveBundleAssetPath,
} from './browser-visual-diff-lib.mjs';

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(SCRIPT_DIR, '..');
const DEFAULT_BASE_DIR = join(REPO_ROOT, 'target', 'browser-visual-diff');

const SCENES = {
  'globe-world': {
    version: 8,
    projection: { type: 'globe' },
    center: [0.0, 25.0],
    zoom: 1.2,
    sky: {
      'sky-color': '#183b6b',
      'sky-horizon-blend': 0.5,
      'horizon-color': '#7ab5e6',
      'atmosphere-blend': 0.85,
    },
    sources: {
      land: {
        type: 'geojson',
        data: {
          type: 'FeatureCollection',
          features: [
            {
              type: 'Feature',
              properties: { kind: 'americas' },
              geometry: {
                type: 'Polygon',
                coordinates: [[[-100, 10], [-45, 10], [-45, 55], [-100, 55], [-100, 10]]],
              },
            },
            {
              type: 'Feature',
              properties: { kind: 'antimeridian' },
              geometry: {
                type: 'Polygon',
                coordinates: [[[165, -25], [195, -25], [195, 25], [165, 25], [165, -25]]],
              },
            },
            {
              type: 'Feature',
              properties: { kind: 'polar' },
              geometry: {
                type: 'Polygon',
                coordinates: [[[-30, 62], [60, 62], [60, 78], [-30, 78], [-30, 62]]],
              },
            },
            {
              type: 'Feature',
              properties: { kind: 'equator-track' },
              geometry: {
                type: 'LineString',
                coordinates: [[-150, 0], [0, 0], [150, 0]],
              },
            },
          ],
        },
      },
    },
    layers: [
      { id: 'bg', type: 'background', paint: { 'background-color': '#0b2545' } },
      { id: 'land-fill', type: 'fill', source: 'land', paint: { 'fill-color': '#4caf50', 'fill-opacity': 0.9 } },
      { id: 'land-line', type: 'line', source: 'land', paint: { 'line-color': '#ffd166', 'line-width': 2.5 } },
    ],
  },
  'mercator-vectors': {
    version: 8,
    projection: { type: 'mercator' },
    center: [0.0, 0.0],
    zoom: 2.2,
    sources: {
      shapes: {
        type: 'geojson',
        data: {
          type: 'FeatureCollection',
          features: [
            {
              type: 'Feature',
              geometry: {
                type: 'Polygon',
                coordinates: [[[-35, -25], [35, -25], [35, 25], [-35, 25], [-35, -25]]],
              },
              properties: {},
            },
            {
              type: 'Feature',
              geometry: {
                type: 'LineString',
                coordinates: [[-55, -35], [0, 35], [55, -35]],
              },
              properties: {},
            },
            {
              type: 'Feature',
              geometry: { type: 'Point', coordinates: [0, 0] },
              properties: {},
            },
            {
              type: 'Feature',
              geometry: { type: 'Point', coordinates: [-40, 20] },
              properties: {},
            },
          ],
        },
      },
    },
    layers: [
      { id: 'bg', type: 'background', paint: { 'background-color': '#1b263b' } },
      {
        id: 'poly',
        type: 'fill',
        source: 'shapes',
        filter: ['==', ['geometry-type'], 'Polygon'],
        paint: { 'fill-color': '#2ec4b6' },
      },
      {
        id: 'stroke',
        type: 'line',
        source: 'shapes',
        filter: ['==', ['geometry-type'], 'LineString'],
        paint: { 'line-color': '#ff9f1c', 'line-width': 4.0 },
      },
      {
        id: 'dots',
        type: 'circle',
        source: 'shapes',
        filter: ['==', ['geometry-type'], 'Point'],
        paint: { 'circle-color': '#e71d36', 'circle-radius': 14.0, 'circle-stroke-width': 2.0, 'circle-stroke-color': '#ffffff' },
      },
    ],
  },
};

function mimeTypeFor(filePath) {
  switch (extname(filePath).toLowerCase()) {
    case '.wasm':
      return 'application/wasm';
    case '.js':
    case '.mjs':
      return 'application/javascript; charset=utf-8';
    case '.html':
      return 'text/html; charset=utf-8';
    case '.json':
      return 'application/json; charset=utf-8';
    case '.css':
      return 'text/css; charset=utf-8';
    default:
      return 'application/octet-stream';
  }
}

async function loadPlaywrightCore() {
  const candidates = [];
  if (process.env.PLAYWRIGHT_CORE_PATH) {
    candidates.push(process.env.PLAYWRIGHT_CORE_PATH);
  }
  candidates.push('playwright-core');
  try {
    const globalRoot = execFileSync('npm', ['root', '-g'], { encoding: 'utf8' }).trim();
    if (globalRoot) {
      candidates.push(join(globalRoot, 'playwright-core', 'index.mjs'));
    }
  } catch {
    // Ignore global npm probe failure.
  }
  for (const candidate of candidates) {
    try {
      return candidate.startsWith('/')
        ? await import(pathToFileURL(candidate).href)
        : await import(candidate);
    } catch {
      // Try next candidate.
    }
  }
  throw new Error(
    'Unable to resolve playwright-core. Install globally (`npm i -g playwright-core`) or set PLAYWRIGHT_CORE_PATH.',
  );
}

function parseArgs(argv) {
  const opts = {
    webgpuDir: join(DEFAULT_BASE_DIR, 'bundles', 'webgpu'),
    webglDir: join(DEFAULT_BASE_DIR, 'bundles', 'webgl'),
    outDir: join(DEFAULT_BASE_DIR, 'report'),
    browsers: ['chromium', 'firefox'],
    maxDiffRatio: 0.02,
    channelThreshold: 10,
    allowBrowserSkip: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--webgpu-dir') opts.webgpuDir = resolve(argv[++i] ?? '');
    else if (arg === '--webgl-dir') opts.webglDir = resolve(argv[++i] ?? '');
    else if (arg === '--out-dir') opts.outDir = resolve(argv[++i] ?? '');
    else if (arg === '--browsers') {
      opts.browsers = (argv[++i] ?? '')
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean);
    } else if (arg === '--max-diff-ratio') {
      opts.maxDiffRatio = Number(argv[++i]);
    } else if (arg === '--channel-threshold') {
      opts.channelThreshold = Number(argv[++i]);
    } else if (arg === '--allow-browser-skip') {
      opts.allowBrowserSkip = true;
    } else {
      throw new Error(`Unknown CLI argument: ${arg}`);
    }
  }
  return opts;
}

function harnessHtml() {
  return `<!DOCTYPE html>
<html lang="en" style="margin:0;padding:0;width:512px;height:512px;overflow:hidden;">
<body style="margin:0;padding:0;width:512px;height:512px;overflow:hidden;background:#000;">
<canvas id="maplibre" width="512" height="512" style="display:block;width:512px;height:512px;"></canvas>
<script type="module">
  const params = new URLSearchParams(location.search);
  const backend = params.get('backend');
  const sceneName = params.get('scene');
  window.__diffState = { status: 'booting', activeBackend: null, error: null };

  const rafWait = (frames = 20) => new Promise((resolve) => {
    let n = 0;
    const step = () => (++n >= frames ? resolve() : requestAnimationFrame(step));
    requestAnimationFrame(step);
  });

  try {
    const styleResp = await fetch('/scenes/' + encodeURIComponent(sceneName) + '.json');
    if (!styleResp.ok) throw new Error('Missing scene: ' + sceneName);
    const styleJson = await styleResp.text();

    if (backend === 'webgpu') {
      const adapter = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
      if (!adapter) {
        window.__diffState = { status: 'no-webgpu-adapter', activeBackend: null, error: 'navigator.gpu.requestAdapter() returned null' };
        throw new Error('SKIP_NO_WEBGPU');
      }
    }

    const mod = await import('/' + backend + '/module.js');
    let fallbackDetected = false;
    const origWarn = console.warn.bind(console);
    console.warn = (...args) => {
      if (String(args[0] ?? '').includes('falling back automatically to WebGL2')) {
        fallbackDetected = true;
      }
      origWarn(...args);
    };

    const options = backend === 'webgpu' ? { webglModulePath: '/blocked-webgl-fallback.js' } : undefined;
    const map = await mod.startMapLibre(undefined, undefined, styleJson, [], 'maplibre', options);
    if (!map) {
      throw new Error('startMapLibre returned undefined (fallbackDetected=' + fallbackDetected + ')');
    }
    if (backend === 'webgpu' && fallbackDetected) {
      throw new Error('Silent fallback to WebGL2 detected on WebGPU run');
    }
    await map.getCamera();
    await rafWait(25);
    window.__diffState = { status: 'ready', activeBackend: backend, error: null };
  } catch (err) {
    if (window.__diffState.status !== 'no-webgpu-adapter') {
      window.__diffState = {
        status: 'error',
        activeBackend: null,
        error: err instanceof Error ? err.message : String(err),
      };
    }
  }
</script>
</body>
</html>`;
}

function startStaticServer({ webgpuDir, webglDir }) {
  const server = createServer((req, res) => {
    const urlPath = (req.url ?? '/').split('#')[0].split('?')[0];
    if (urlPath === '/harness.html') {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(harnessHtml());
      return;
    }
    if (urlPath.startsWith('/scenes/') && urlPath.endsWith('.json')) {
      const name = urlPath.slice('/scenes/'.length, -'.json'.length);
      const scene = SCENES[name];
      if (!scene) {
        res.writeHead(404);
        res.end('Not found');
        return;
      }
      res.writeHead(200, { 'content-type': 'application/json; charset=utf-8' });
      res.end(JSON.stringify(scene));
      return;
    }
    for (const [prefix, dir] of [
      ['/webgpu/', webgpuDir],
      ['/webgl/', webglDir],
    ]) {
      if (urlPath.startsWith(prefix)) {
        const subUrl = '/' + urlPath.slice(prefix.length);
        const filePath = resolveBundleAssetPath(dir, subUrl);
        if (!filePath || !existsSync(filePath) || !statSync(filePath).isFile()) {
          res.writeHead(404);
          res.end('Not found');
          return;
        }
        res.writeHead(200, { 'content-type': mimeTypeFor(filePath) });
        res.end(readFileSync(filePath));
        return;
      }
    }
    res.writeHead(404);
    res.end('Not found');
  });

  return new Promise((resolvePromise, rejectPromise) => {
    server.once('error', rejectPromise);
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      resolvePromise({ server, baseUrl: `http://127.0.0.1:${addr.port}` });
    });
  });
}

async function resolveExecutable(candidates) {
  for (const candidate of candidates) {
    if (candidate && existsSync(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

async function loadPuppeteerCore() {
  const candidates = [];
  if (process.env.PUPPETEER_CORE_PATH) {
    candidates.push(process.env.PUPPETEER_CORE_PATH);
  }
  candidates.push('puppeteer-core');
  try {
    const globalRoot = execFileSync('npm', ['root', '-g'], { encoding: 'utf8' }).trim();
    if (globalRoot) {
      candidates.push(join(globalRoot, 'puppeteer-core', 'lib', 'puppeteer', 'puppeteer-core.js'));
    }
  } catch {
    // Ignore global npm probe failure.
  }
  for (const candidate of candidates) {
    try {
      const mod = candidate.startsWith('/')
        ? await import(pathToFileURL(candidate).href)
        : await import(candidate);
      return mod.default ?? mod;
    } catch {
      // Try next candidate.
    }
  }
  throw new Error(
    'Unable to resolve puppeteer-core for Firefox BiDi fallback. Install globally (`npm i -g puppeteer-core`).',
  );
}

const BROWSER_LAUNCH_TIMEOUT_MS = 15_000;

async function withLaunchTimeout(promise, label, timeoutMs = BROWSER_LAUNCH_TIMEOUT_MS + 3_000) {
  let timedOut = false;
  let timer;
  const timeoutPromise = new Promise((_, reject) => {
    timer = setTimeout(() => {
      timedOut = true;
      reject(new Error(`${label} failed to launch within ${timeoutMs}ms (headless session or binary unavailable)`));
    }, timeoutMs);
  });
  try {
    return await Promise.race([
      promise.then((session) => {
        if (timedOut) {
          session?.browser?.close?.().catch(() => {});
        }
        return session;
      }),
      timeoutPromise,
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function launchBrowser(pw, name) {
  if (name === 'chromium') {
    let managed = '';
    try {
      managed = pw.chromium.executablePath();
    } catch {
      managed = '';
    }
    const executablePath = await resolveExecutable([
      process.env.CHROME_EXECUTABLE_PATH,
      managed,
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      '/usr/bin/google-chrome',
      '/usr/bin/chromium',
      '/usr/bin/chromium-browser',
    ]);
    const browser = await pw.chromium.launch({
      headless: true,
      timeout: BROWSER_LAUNCH_TIMEOUT_MS,
      ...(executablePath ? { executablePath } : {}),
      args: ['--enable-unsafe-webgpu', '--ignore-gpu-blocklist'],
    });
    return { kind: 'playwright', browser };
  }
  if (name === 'firefox') {
    let managed = '';
    try {
      managed = pw.firefox.executablePath();
    } catch {
      managed = '';
    }
    if (managed && existsSync(managed)) {
      const browser = await pw.firefox.launch({
        headless: true,
        timeout: BROWSER_LAUNCH_TIMEOUT_MS,
        firefoxUserPrefs: {
          'dom.webgpu.enabled': true,
          'gfx.webgpu.ignore-blocklist': true,
        },
      });
      return { kind: 'playwright', browser };
    }
    const executablePath = await resolveExecutable([
      process.env.FIREFOX_EXECUTABLE_PATH,
      '/Applications/Firefox.app/Contents/MacOS/firefox',
      '/usr/bin/firefox',
    ]);
    const ppt = await loadPuppeteerCore();
    const browser = await ppt.launch({
      browser: 'firefox',
      headless: true,
      timeout: BROWSER_LAUNCH_TIMEOUT_MS,
      ...(executablePath ? { executablePath } : {}),
      extraPrefsFirefox: {
        'dom.webgpu.enabled': true,
        'gfx.webgpu.ignore-blocklist': true,
      },
    });
    return { kind: 'puppeteer', browser };
  }
  throw new Error(`Unsupported browser "${name}"; expected chromium or firefox`);
}

async function captureRun({ session, browserName, baseUrl, backend, sceneName, outDir }) {
  const isPlaywright = session.kind === 'playwright';
  const page = isPlaywright
    ? await session.browser.newPage({ viewport: { width: 512, height: 512 }, deviceScaleFactor: 1 })
    : await session.browser.newPage();
  if (!isPlaywright) {
    await page.setViewport({ width: 512, height: 512, deviceScaleFactor: 1 });
  }
  const consoleErrors = [];
  page.on('pageerror', (err) => consoleErrors.push(`pageerror: ${err.message}`));
  page.on('console', (msg) => {
    const type = typeof msg.type === 'function' ? msg.type() : msg.type;
    if (type === 'error') {
      const text = typeof msg.text === 'function' ? msg.text() : '';
      consoleErrors.push(`console.error: ${text}`);
    }
  });

  try {
    await page.goto(`${baseUrl}/harness.html?backend=${backend}&scene=${sceneName}`, {
      waitUntil: 'domcontentloaded',
    });
    await page.waitForFunction(
      () => window.__diffState && window.__diffState.status !== 'booting',
      isPlaywright ? null : {},
      { timeout: 30_000 },
    );
    const state = await page.evaluate(() => window.__diffState);
    if (state.status === 'no-webgpu-adapter') {
      return {
        browser: browserName,
        scene: sceneName,
        backend,
        activeBackend: null,
        content: null,
        status: 'skipped-no-webgpu',
        error: state.error,
        decoded: null,
      };
    }
    if (state.status !== 'ready') {
      throw new Error(
        `${browserName}/${backend}/${sceneName} failed: ${state.error ?? state.status} | logs=${consoleErrors.join('; ')}`,
      );
    }

    let pngBuffer;
    if (isPlaywright) {
      pngBuffer = await page.locator('#maplibre').screenshot({ type: 'png' });
    } else {
      const el = await page.$('#maplibre');
      pngBuffer = await el.screenshot({ type: 'png' });
    }
    const pngPath = join(outDir, `${browserName}-${backend}-${sceneName}.png`);
    writeFileSync(pngPath, pngBuffer);
    const decoded = decodePng(pngBuffer);
    const content = analyzeImageContent(decoded.rgba, decoded.width, decoded.height);
    if (content.isUniform || content.distinctSampleColors < 3) {
      throw new Error(
        `${browserName}/${backend}/${sceneName} produced blank/uniform image (${content.distinctSampleColors} colors, span ${content.maxChannelSpan})`,
      );
    }

    return {
      browser: browserName,
      scene: sceneName,
      backend,
      activeBackend: state.activeBackend,
      content,
      status: 'ok',
      pngPath,
      decoded,
    };
  } finally {
    await page.close();
  }
}

export async function runVisualDiff(argv = process.argv.slice(2)) {
  const opts = parseArgs(argv);
  for (const [label, dir] of [
    ['webgpu', opts.webgpuDir],
    ['webgl', opts.webglDir],
  ]) {
    if (!existsSync(join(dir, 'module.js'))) {
      throw new Error(`Missing ${label} bundle module.js in ${dir}. Build release bundles first.`);
    }
  }

  mkdirSync(opts.outDir, { recursive: true });
  const pw = await loadPlaywrightCore();
  const { server, baseUrl } = await startStaticServer({
    webgpuDir: opts.webgpuDir,
    webglDir: opts.webglDir,
  });

  const captures = [];
  try {
    for (const browserName of opts.browsers) {
      let session;
      try {
        session = await withLaunchTimeout(launchBrowser(pw, browserName), browserName);
      } catch (err) {
        if (!opts.allowBrowserSkip) {
          throw err;
        }
        const message = err instanceof Error ? err.message : String(err);
        captures.push({
          browser: browserName,
          scene: '*',
          backend: '*',
          activeBackend: null,
          content: null,
          status: 'skipped-launch-error',
          error: message,
          decoded: null,
        });
        continue;
      }
      try {
        for (const sceneName of Object.keys(SCENES)) {
          for (const backend of ['webgpu', 'webgl']) {
            const cap = await captureRun({
              session,
              browserName,
              baseUrl,
              backend,
              sceneName,
              outDir: opts.outDir,
            });
            captures.push(cap);
          }
        }
      } finally {
        await session.browser.close();
      }
    }
  } finally {
    await new Promise((resolveClose) => server.close(resolveClose));
  }

  const byKey = new Map(captures.map((c) => [`${c.browser}:${c.backend}:${c.scene}`, c]));
  const comparisons = [];
  const comparePair = (label, aKey, bKey, scene) => {
    const a = byKey.get(aKey);
    const b = byKey.get(bKey);
    if (!a?.decoded || !b?.decoded) return;
    if (a.decoded.width !== b.decoded.width || a.decoded.height !== b.decoded.height) {
      throw new Error(`Dimension mismatch between ${aKey} and ${bKey}`);
    }
    const cmp = compareRgbaBuffers(a.decoded.rgba, b.decoded.rgba, a.decoded.width, a.decoded.height, {
      channelThreshold: opts.channelThreshold,
    });
    const ok = cmp.diffRatio <= opts.maxDiffRatio;
    const diffPngName = `diff-${label.replace(/[^a-z0-9]+/gi, '-').toLowerCase()}-${scene}.png`;
    writeFileSync(
      join(opts.outDir, diffPngName),
      encodeRgbaPng(a.decoded.width, a.decoded.height, cmp.diffRgba),
    );
    comparisons.push({
      label,
      scene,
      diffPixels: cmp.diffPixels,
      totalPixels: cmp.totalPixels,
      diffRatio: cmp.diffRatio,
      maxChannelDiff: cmp.maxChannelDiff,
      meanAbsDiff: cmp.meanAbsDiff,
      ok,
      diffPngName,
    });
  };

  for (const scene of Object.keys(SCENES)) {
    for (const browserName of opts.browsers) {
      comparePair(
        `${browserName}:webgpu vs ${browserName}:webgl`,
        `${browserName}:webgpu:${scene}`,
        `${browserName}:webgl:${scene}`,
        scene,
      );
    }
    if (opts.browsers.includes('chromium') && opts.browsers.includes('firefox')) {
      comparePair(
        'chromium:webgl vs firefox:webgl',
        `chromium:webgl:${scene}`,
        `firefox:webgl:${scene}`,
        scene,
      );
      comparePair(
        'chromium:webgpu vs firefox:webgpu',
        `chromium:webgpu:${scene}`,
        `firefox:webgpu:${scene}`,
        scene,
      );
    }
  }

  const summary = {
    generatedAt: new Date().toISOString(),
    maxDiffRatio: opts.maxDiffRatio,
    channelThreshold: opts.channelThreshold,
    captures: captures.map(({ decoded: _decoded, ...rest }) => rest),
    comparisons,
  };
  writeFileSync(join(opts.outDir, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`);
  const markdown = formatDiffReport(summary);
  writeFileSync(join(opts.outDir, 'summary.md'), markdown);
  console.log(markdown);

  if (comparisons.length === 0) {
    throw new Error('No comparable browser/backend capture pairs completed');
  }
  const failed = comparisons.filter((c) => !c.ok);
  if (failed.length > 0) {
    throw new Error(
      `Visual diff exceeded threshold ${(opts.maxDiffRatio * 100).toFixed(2)}% in: ` +
        failed.map((f) => `${f.label} (${f.scene}: ${(f.diffRatio * 100).toFixed(2)}%)`).join(', '),
    );
  }
  return summary;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  runVisualDiff().catch((err) => {
    console.error(err);
    process.exitCode = 1;
  });
}
