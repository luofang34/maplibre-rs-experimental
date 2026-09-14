import './style.css';
import { BrowserMap, mapStyle } from './map.ts';
import { Replay, decodeTrack, sample } from './replay.ts';
import { loadTerrain } from './terrain.ts';
import type { SpatialView } from './spatial.ts';
import type { Action } from './xr-controls.ts';

const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const button = (id: string) => element<HTMLButtonElement>(id);
const now = () => performance.now() / 1000;
const format = (seconds: number) => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`;
const report = (message: string) => { element('status').textContent = message; };
const fail = (error: unknown) => { report(error instanceof Error ? error.message : String(error)); console.error(error); };
window.addEventListener('error', event => fail(event.error ?? event.message));
window.addEventListener('unhandledrejection', event => fail(event.reason));
const style = await mapStyle().catch(error => { fail(error); throw error; });
const browser = new BrowserMap(style);
browser.map.on('error', event => fail(event.error));
let replay: Replay;
let spatial: SpatialView | undefined;
let pending: AbortController | undefined;
let busy = false;
let lastUI = 0;
let supportedXR = false;

async function checkXR() {
  if (!isSecureContext) { button('xr').textContent = 'XR needs HTTPS'; return; }
  try {
    supportedXR = !!navigator.xr && await navigator.xr.isSessionSupported('immersive-vr');
    button('xr').textContent = supportedXR ? 'Enter VR' : 'XR unavailable';
    button('xr').title = supportedXR ? 'Open in your headset' : 'Use Safari on Vision Pro or another WebXR headset. 3D replay works in this browser.';
    button('xr').disabled = !supportedXR || !spatial;
  } catch (error) { fail(error); }
}
async function chooseTrack() {
  pending?.abort();
  if (spatial) closeSpatial();
  report('Loading recording…');
  const id = element<HTMLSelectElement>('track').value;
  const response = await fetch(`/data/${id}.json`);
  if (!response.ok) throw new Error(`Recording: HTTP ${response.status}`);
  const track = decodeTrack(await response.text());
  // A slower fetch must not replace the recording selected more recently.
  if (id !== element<HTMLSelectElement>('track').value) return;
  replay = new Replay(track);
  element('title').textContent = track.title ?? track.callsign;
  element('kind').textContent = track.kind === 'simulation' ? 'SIMULATED FLIGHT' : 'RECORDED FLIGHT';
  element('provenance').textContent = track.source.coverage;
  element<HTMLAnchorElement>('source').href = track.source.url;
  element<HTMLSelectElement>('rate').value = '1';
  element<HTMLInputElement>('timeline').max = String(replay.duration);
  browser.setTrack(track); browser.update(sample(track, 0)); report('');
}
function setBusy(value: boolean) {
  busy = value;
  for (const id of ['spatial-open', 'fpv', 'track']) (element(id) as HTMLButtonElement | HTMLSelectElement).disabled = value;
}
async function openSpatial(fpv = false) {
  if (spatial) { fpv ? spatial.fpv() : spatial.overview(); return; }
  if (busy || !replay) return;
  replay.pause(now()); setBusy(true); pending = new AbortController();
  try {
    const { SpatialView } = await import('./spatial.ts');
    const terrain = await loadTerrain(replay.track, style, report, pending.signal);
    element('spatial').hidden = false;
    spatial = new SpatialView(element('spatial'), terrain, replay, action);
    if (fpv) spatial.fpv();
    spatial.renderer.setAnimationLoop((timestamp, frame) => { updateUI(timestamp / 1000); spatial?.render(timestamp / 1000, frame); });
    element('map').hidden = true; element('return-map').hidden = false;
    button('xr').disabled = !supportedXR;
    button('spatial-open').setAttribute('aria-pressed', 'true');
    report('Terrain ready. Drag to orbit; pinch or scroll to zoom. Enter VR on a headset.');
  } catch (error) { pending?.abort(); if (!(error instanceof DOMException && error.name === 'AbortError')) fail(error); }
  finally { pending = undefined; setBusy(false); }
}
function closeSpatial() {
  pending?.abort();
  if (!spatial) return;
  replay.pause(now()); spatial.dispose(); spatial = undefined;
  element('spatial').hidden = true; element('map').hidden = false; element('return-map').hidden = true;
  button('xr').disabled = true; button('spatial-open').setAttribute('aria-pressed', 'false');
  browser.map.resize(); report('');
}
function action(value: Action) {
  if (!replay) return;
  if (value === 'play') replay.playing(now()) ? replay.pause(now()) : replay.play(now());
  if (value === 'overview') spatial ? spatial.overview() : browser.overview(replay.track);
  if (value === 'fpv') openSpatial(true).catch(fail);
  if (value === 'exit') spatial?.exitXR().catch(fail);
}
function updateUI(time: number) {
  if (!replay || time - lastUI < 0.1) return; lastUI = time;
  const elapsed = replay.time(time), p = sample(replay.track, elapsed), playing = replay.playing(time);
  button('play').textContent = playing ? 'Ⅱ Pause' : '▶ Play';
  button('play').setAttribute('aria-label', playing ? 'Pause recording' : 'Play recording');
  element<HTMLInputElement>('timeline').value = String(elapsed);
  element('time').textContent = `${format(elapsed)} / ${format(replay.duration)}`;
  element('telemetry').textContent = p ? `${Math.round(p.altitudeMSL * 3.28084).toLocaleString()} ft MSL  ·  ${p.velocityAvailable === false ? 'GS unavailable' : `${Math.round(p.groundSpeed * 1.94384)} kt GS`}` : 'Position unavailable';
  element('gap').textContent = p ? '' : 'Recording gap';
  if (!spatial) browser.update(p);
  button('overview').setAttribute('aria-pressed', String(!spatial || spatial.mode === 'overview'));
  button('fpv').setAttribute('aria-pressed', String(spatial?.mode === 'fpv'));
}
function animate(timestamp: number) { if (!spatial) updateUI(timestamp / 1000); requestAnimationFrame(animate); }
button('play').onclick = () => action('play');
button('overview').onclick = () => action('overview');
button('fpv').onclick = () => action('fpv');
button('return-map').onclick = closeSpatial;
button('spatial-open').onclick = () => openSpatial().catch(fail);
button('xr').onclick = () => spatial?.enterXR().catch(fail);
button('globe').onclick = () => { closeSpatial(); browser.map.setProjection({ type: 'globe' }); browser.map.easeTo({ zoom: 1.7, pitch: 0, duration: 1000 }); };
button('terrain').onclick = () => { closeSpatial(); browser.map.setProjection({ type: 'globe' }); if (replay) browser.overview(replay.track); };
element<HTMLInputElement>('timeline').oninput = event => replay?.seek(Number((event.target as HTMLInputElement).value), now());
element<HTMLSelectElement>('rate').onchange = event => replay?.setRate(Number((event.target as HTMLSelectElement).value), now());
element<HTMLSelectElement>('track').onchange = () => chooseTrack().catch(fail);
document.addEventListener('visibilitychange', () => { if (document.hidden) replay?.pause(now()); });
window.addEventListener('pagehide', () => { pending?.abort(); spatial?.dispose(); browser.map.remove(); });
await checkXR();
try { await browser.ready; await chooseTrack(); requestAnimationFrame(animate); }
catch (error) { fail(error); }
