import {readFileSync, writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
import {runInNewContext} from 'node:vm';
import {createHash} from 'node:crypto';
const path = process.argv[2];
if (!path) throw Error('Usage: node generate-gljs.mjs /path/to/maplibre-gl-js/src/data/dem_data.ts');
const upstream = readFileSync(path, 'utf8');
const sourceHash = createHash('sha256').update(upstream).digest('hex');
if (sourceHash !== '2f37d12fcac3c3d0e0bbe173e8b437994faf6b313ef299f7d44e6cc5ec2219e2') {
 throw Error('Expected the DEMData source identified by the fixture metadata');
}
const code = upstream.replace(/^import .*;\n/gm, '').replace(/^export /gm, '');
const context = {warnOnce: message => { throw Error(message); }, register: () => {}};
runInNewContext(stripTypeScriptTypes(code) + '\nglobalThis.ReferenceDEM = DEMData;', context);
const DEM = context.ReferenceDEM;
const dim = 4;
const rgba = base => Array.from({length: dim * dim}, (_, i) => {
 const x = i % dim, y = Math.floor(i / dim);
 return [(base + 17 + x * 31 + y * 7) % 256, (251 - x * 17 - y * 31) % 256, (x * 59 + y * 11) % 256, 255];
}).flat();
function create(data, encoding, factors) {
 const stride = dim + 4;
 const padded = new Uint8Array(stride * stride * 4);
 for (let y = 0; y < dim; y++) padded.set(data.slice(y * dim * 4, (y + 1) * dim * 4), ((y + 2) * stride + 2) * 4);
 return new DEM('oracle', {width: stride, height: stride, data: padded}, encoding, ...factors);
}
function samples(dem) {
 const result = [];
 for (let y = -2; y < dim + 2; y++) for (let x = -2; x < dim + 2; x++) result.push(dem.get(x, y));
 return result;
}
const offsets = [[-1,-1],[0,-1],[1,-1],[-1,0],[1,0],[-1,1],[0,1],[1,1]];
const coordinates = [[0,0],[0.125,0.125],[0.25,0.25],[0.5,0.5],[0.75,0.75],[0.875,0.875],[1,1],[0,0.5],[1,0.5]];
const cases = [
 ['mapbox', []], ['terrarium', []], ['custom', [0.25,64,16384,7000]], ['custom', [0,0,0,17]],
].map(([encoding, factors]) => {
 const data = rgba(0), neighbour = rgba(79), dem = create(data, encoding, factors);
 return {encoding, unpack: dem.getUnpackVector(), dim, rgba: data, neighbour_rgba: neighbour,
 min: dem.min, max: dem.max, initial: samples(dem),
 tile_samples: coordinates.map(([x,y]) => [x,y,dem.sampleBilinear(x * dim - 0.5, y * dim - 0.5)]),
 backfilled: offsets.map(([dx,dy]) => {
  const tile = create(data, encoding, factors); tile.backfillBorder(create(neighbour, encoding, factors), dx, dy);
  return {dx,dy,samples:samples(tile)};
 })};
});
const result = {source:'https://github.com/maplibre/maplibre-gl-js/blob/v6.11.2/src/data/dem_data.ts',
 source_sha256:sourceHash,
 sampling_source:'https://github.com/maplibre/maplibre-gl-js/blob/v6.11.2/src/render/terrain.ts',cases};
writeFileSync(new URL('./gljs.json', import.meta.url), JSON.stringify(result) + '\n');
console.log(`Generated ${cases.length} encodings, ${cases.length * offsets.length} neighbor cases from GL JS DEMData.`);
