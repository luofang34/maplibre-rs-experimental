import * as maplibregl from 'maplibre-gl';
import mapWorkerURL from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';
import type { StyleSpecification, GeoJSONSource } from 'maplibre-gl';
import { segments, type Track, type Observation } from './replay.ts';

export async function mapStyle(): Promise<StyleSpecification> {
  const response = await fetch('/data/terrain-style.json');
  if (!response.ok) throw new Error(`Map style: HTTP ${response.status}`);
  return response.json();
}
export function bounds(track: Track): [[number, number], [number, number]] {
  const p = track.observations;
  return [[Math.min(...p.map(p => p.longitude)), Math.min(...p.map(p => p.latitude))], [Math.max(...p.map(p => p.longitude)), Math.max(...p.map(p => p.latitude))]];
}
export class BrowserMap {
  map: maplibregl.Map;
  marker: maplibregl.Marker;
  ready: Promise<void>;
  constructor(style: StyleSpecification) {
    maplibregl.setWorkerUrl(mapWorkerURL);
    maplibregl.setWorkerCount(2);
    this.map = new maplibregl.Map({ container: 'map', style, center: [11.7, 47.35], zoom: 8.5, pitch: 50, maxPitch: 80, maxTileCacheSize: 128, canvasContextAttributes: { antialias: true }, attributionControl: false });
    this.ready = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('Map startup timed out. Check network access and reload.')), 45000);
      this.map.once('style.load', () => { clearTimeout(timeout); resolve(); });
      this.map.once('error', e => { clearTimeout(timeout); reject(e.error); });
    });
    const element = document.createElement('div'); element.className = 'ownship'; element.textContent = '▲';
    this.marker = new maplibregl.Marker({ element, rotationAlignment: 'map', pitchAlignment: 'map' });
  }
  setTrack(track: Track) {
    const data = { type: 'FeatureCollection' as const, features: segments(track).map(s => ({ type: 'Feature' as const, properties: {}, geometry: { type: 'LineString' as const, coordinates: s.map(p => [p.longitude, p.latitude, p.altitudeMSL]) } })) };
    const source = this.map.getSource('flight') as GeoJSONSource | undefined;
    if (source) source.setData(data);
    else {
      this.map.addSource('flight', { type: 'geojson', data });
      this.map.addLayer({ id: 'flight-halo', type: 'line', source: 'flight', paint: { 'line-color': '#163d3a', 'line-width': 6, 'line-opacity': 0.6 }, layout: { 'line-join': 'round', 'line-cap': 'round' } });
      this.map.addLayer({ id: 'flight', type: 'line', source: 'flight', paint: { 'line-color': '#48ddd4', 'line-width': 3 }, layout: { 'line-join': 'round', 'line-cap': 'round' } });
    }
    this.overview(track);
  }
  overview(track: Track) { this.map.fitBounds(bounds(track), { padding: { top: 110, left: innerWidth > 760 ? 320 : 40, right: 55, bottom: 235 }, pitch: 45, duration: 900, maxZoom: 11 }); }
  update(p?: Observation) {
    if (!p) { this.marker.remove(); return; }
    this.marker.setLngLat([p.longitude, p.latitude]).setRotation(p.track).addTo(this.map);
  }
}
