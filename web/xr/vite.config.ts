import { defineConfig, type Plugin } from 'vite';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const native = new URL('../../apple/visionos/MapLibreVision/MapLibreVision/', import.meta.url);
const assets = new Map([
  ['terrain-style.json', new URL('style.json', native)],
  ['innsbruck-approach.json', new URL('Resources/innsbruck-approach.json', native)],
  ['mach-loop.json', new URL('Resources/mach-loop.json', native)],
]);

function sharedAssets(): Plugin {
  return {
    name: 'shared-map-and-replay-assets',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const path = assets.get((req.url ?? '').split('?')[0].replace('/data/', ''));
        if (!path || !req.url?.startsWith('/data/')) return next();
        res.setHeader('Content-Type', 'application/json');
        res.end(readFileSync(path));
      });
    },
    generateBundle() {
      for (const [name, path] of assets) {
        this.emitFile({ type: 'asset', fileName: `data/${name}`, source: readFileSync(path) });
      }
    },
  };
}

const cert = fileURLToPath(new URL('.cert/server.pem', import.meta.url));
const key = fileURLToPath(new URL('.cert/server-key.pem', import.meta.url));
const https = process.env.XR_HTTPS === '1';
if (https && (!existsSync(cert) || !existsSync(key))) throw new Error('Run npm run cert first.');
export default defineConfig({
  plugins: [sharedAssets()],
  optimizeDeps: { exclude: ['maplibre-gl'] },
  server: { port: 5174, strictPort: true, https: https ? { cert: readFileSync(cert), key: readFileSync(key) } : undefined },
  preview: { port: 4174, strictPort: true, https: https ? { cert: readFileSync(cert), key: readFileSync(key) } : undefined },
  build: { target: 'es2022' },
});
