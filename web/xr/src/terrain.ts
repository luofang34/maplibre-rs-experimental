import * as maplibregl from 'maplibre-gl';
import type { StyleSpecification } from 'maplibre-gl';
import * as THREE from 'three';
import { bounds } from './map.ts';
import { geographic, mercator, WORLD, radians, clamp } from './geo.ts';
import type { Observation, Track } from './replay.ts';

export interface Terrain {
  mesh: THREE.Mesh<THREE.PlaneGeometry, THREE.MeshStandardMaterial>;
  width: number;
  toLocal(p: Pick<Observation, 'latitude' | 'longitude' | 'altitudeMSL'>): THREE.Vector3;
  dispose(): void;
}
// Heights and color share exactly the same Mercator footprint. One continuous grid avoids
// independently simplified tile borders, while loading before playback avoids LOD churn.
export async function loadTerrain(track: Track, style: StyleSpecification, report: (s: string) => void, signal: AbortSignal): Promise<Terrain> {
  signal = AbortSignal.any([signal, AbortSignal.timeout(60000)]);
  signal.throwIfAborted();
  const container = document.createElement('div');
  container.className = 'terrain-baker'; document.body.append(container);
  const surfaceStyle = structuredClone(style);
  delete surfaceStyle.terrain; delete surfaceStyle.sky;
  surfaceStyle.projection = { type: 'mercator' };
  surfaceStyle.layers = surfaceStyle.layers.filter(l => l.type !== 'symbol' && l.type !== 'fill-extrusion' && l.type !== 'hillshade');
  const baker = new maplibregl.Map({ container, style: surfaceStyle, pitch: 0, bearing: 0, interactive: false, attributionControl: false, pixelRatio: 1, fadeDuration: 0, canvasContextAttributes: { preserveDrawingBuffer: true }, maxTileCacheSize: 64 });
  let texture: THREE.CanvasTexture | undefined;
  try {
    report('Preparing the map surface…');
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => finish(new Error('Map surface timed out. Check tile access and retry.')), 45000);
      const abort = () => finish(new DOMException('Cancelled', 'AbortError'));
      const finish = (error?: Error) => { clearTimeout(timeout); signal.removeEventListener('abort', abort); error ? reject(error) : resolve(); };
      signal.addEventListener('abort', abort, { once: true });
      baker.once('load', () => {
        baker.fitBounds(bounds(track), { padding: 260, pitch: 0, bearing: 0, animate: false, maxZoom: 12 });
        baker.once('idle', () => finish());
      });
      baker.on('error', e => finish(new Error(`Map surface: ${e.error.message}`)));
    });
    signal.throwIfAborted();
    const nw = baker.unproject([0, 0]), se = baker.unproject([2048, 2048]);
    const [x0, y0] = mercator(nw.lng, nw.lat), [x1, y1] = mercator(se.lng, se.lat);
    const center = [(x0 + x1) / 2, (y0 + y1) / 2];
    const meters = WORLD * Math.cos(radians(geographic(...center as [number, number])[1]));
    const width = (x1 - x0) * meters;
    const image = document.createElement('canvas'); image.width = image.height = 2048;
    const context = image.getContext('2d'); if (!context) throw new Error('Cannot allocate map surface.');
    context.drawImage(baker.getCanvas(), 0, 0);
    texture = new THREE.CanvasTexture(image); texture.colorSpace = THREE.SRGBColorSpace; texture.anisotropy = 4;
    baker.remove(); container.remove();
    report('Loading terrain for the complete recording…');
    const elevations = await elevationGrid(x0, y0, x1, y1, signal, report);
    signal.throwIfAborted();
    const geometry = new THREE.PlaneGeometry(width, (y1 - y0) * meters, 256, 256);
    geometry.rotateX(-Math.PI / 2);
    const positions = geometry.attributes.position;
    for (let i = 0; i < positions.count; i++) positions.setY(i, elevations[i]);
    geometry.computeVertexNormals();
    const material = new THREE.MeshStandardMaterial({ map: texture, roughness: 1, metalness: 0, side: THREE.DoubleSide });
    const mesh = new THREE.Mesh(geometry, material);
    return { mesh, width, toLocal(p) { const [x, y] = mercator(p.longitude, p.latitude); return new THREE.Vector3((x - center[0]) * meters, p.altitudeMSL, (y - center[1]) * meters); }, dispose() { geometry.dispose(); material.dispose(); texture?.dispose(); image.width = image.height = 1; } };
  } catch (error) { texture?.dispose(); throw error; }
  finally { if (container.isConnected) { baker.remove(); container.remove(); } }
}

async function elevationGrid(x0: number, y0: number, x1: number, y1: number, signal: AbortSignal, report: (s: string) => void) {
  let z = 12;
  const tileCount = () => (Math.floor(x1 * 2 ** z) - Math.floor(x0 * 2 ** z) + 1) * (Math.floor(y1 * 2 ** z) - Math.floor(y0 * 2 ** z) + 1);
  while (z > 0 && tileCount() > 48) z--;
  const n = 2 ** z, tx = Math.floor(x0 * n), ty = Math.floor(y0 * n);
  const columns = Math.floor(x1 * n) - tx + 1, rows = Math.floor(y1 * n) - ty + 1;
  const tiles = new Map<number, Uint8ClampedArray>();
  let next = 0, done = 0;
  await Promise.all(Array.from({ length: 4 }, async () => {
    while (next < columns * rows) {
      const index = next++, x = tx + index % columns, y = ty + Math.floor(index / columns);
      const url = `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/${z}/${((x % n) + n) % n}/${clamp(y, 0, n - 1)}.png`;
      const response = await fetch(url, { signal });
      if (!response.ok) throw new Error(`Terrain tile ${z}/${x}/${y}: HTTP ${response.status}`);
      const bitmap = await createImageBitmap(await response.blob());
      try {
        const canvas = new OffscreenCanvas(256, 256), ctx = canvas.getContext('2d');
        if (!ctx) throw new Error('Cannot decode terrain.');
        ctx.drawImage(bitmap, 0, 0); tiles.set(index, ctx.getImageData(0, 0, 256, 256).data);
      } finally { bitmap.close(); }
      report(`Loading terrain ${++done}/${columns * rows}…`);
    }
  }));
  const height = (px: number, py: number) => {
    px = clamp(px, 0, columns * 256 - 1); py = clamp(py, 0, rows * 256 - 1);
    const data = tiles.get(Math.floor(py / 256) * columns + Math.floor(px / 256))!;
    const i = ((py % 256) * 256 + px % 256) * 4;
    return data[i] * 256 + data[i + 1] + data[i + 2] / 256 - 32768;
  };
  const result = new Float32Array(257 * 257);
  for (let row = 0; row <= 256; row++) for (let col = 0; col <= 256; col++) {
    const px = ((x0 + (x1 - x0) * col / 256) * n - tx) * 256;
    const py = ((y0 + (y1 - y0) * row / 256) * n - ty) * 256;
    const ix = Math.floor(px), iy = Math.floor(py), fx = px - ix, fy = py - iy;
    const top = height(ix, iy) * (1 - fx) + height(ix + 1, iy) * fx;
    const bottom = height(ix, iy + 1) * (1 - fx) + height(ix + 1, iy + 1) * fx;
    result[row * 257 + col] = clamp(top * (1 - fy) + bottom * fy, -500, 9000);
  }
  return result;
}
