import { angle, clamp, distance, radians, degrees } from './geo.ts';

export interface Observation {
  time: number; latitude: number; longitude: number; altitudeMSL: number;
  groundSpeed: number; track: number; roll?: number; pitch?: number; heading?: number;
  indicatedAirspeed?: number; startsSegment?: boolean; velocityAvailable?: boolean;
}
export interface Track {
  title?: string; kind?: 'recorded' | 'simulation'; callsign: string;
  source: { url: string; license: string; coverage: string };
  observations: Observation[];
}
const bounded = (v: unknown, lo: number, hi: number): v is number => typeof v === 'number' && Number.isFinite(v) && v >= lo && v <= hi;
export function decodeTrack(text: string): Track {
  if (new TextEncoder().encode(text).length > 8 * 1024 * 1024) throw new Error('Track exceeds 8 MB.');
  const value = JSON.parse(text) as Track;
  if (!value || !Array.isArray(value.observations) || value.observations.length < 2 || value.observations.length > 20000) throw new Error('Track needs 2–20,000 observations.');
  if (typeof value.callsign !== 'string' || typeof value.source?.coverage !== 'string') throw new Error('Track provenance is missing.');
  if (!/^https:\/\//.test(value.source.url)) throw new Error('Track source must be an HTTPS URL.');
  let previous: Observation | undefined;
  for (const p of value.observations) {
    if (!p || !bounded(p.time, 0, 259200) || (!previous && p.time !== 0) || (previous && p.time <= previous.time) ||
        !bounded(p.latitude, -84, 84) || !bounded(p.longitude, -180, 180) || !bounded(p.altitudeMSL, -500, 20000) ||
        !bounded(p.groundSpeed, 0, 500) || !bounded(p.track, 0, 360)) throw new Error('Invalid track position or timestamp.');
    for (const [field, low, high] of [['roll', -180, 180], ['pitch', -90, 90], ['heading', 0, 360], ['indicatedAirspeed', 0, 500]] as const) {
      if (p[field] != null && !bounded(p[field], low, high)) throw new Error(`Invalid ${field}.`);
    }
    if (previous && connected(previous, p) && distance(previous, p) > (p.time - previous.time) * 500 + 300) throw new Error('Implausible track jump.');
    previous = p;
  }
  return value;
}
export const connected = (a: Observation, b: Observation) => b.time - a.time <= 20 && b.startsSegment !== true;
export function sample(track: Track, seconds: number): Observation | undefined {
  if (!Number.isFinite(seconds)) return;
  const points = track.observations;
  const time = clamp(seconds, 0, points.at(-1)!.time);
  let low = 0, high = points.length - 1;
  while (low + 1 < high) { const mid = (low + high) >> 1; if (points[mid].time <= time) low = mid; else high = mid; }
  const a = points[low], b = points[high];
  if (time === a.time) return a;
  if (time === b.time) return b;
  if (!connected(a, b)) return;
  const t = (time - a.time) / (b.time - a.time);
  const lerp = (x: number, y: number) => x + (y - x) * t;
  const unit = (p: Observation) => [Math.cos(radians(p.latitude)) * Math.cos(radians(p.longitude)), Math.sin(radians(p.latitude)), Math.cos(radians(p.latitude)) * Math.sin(radians(p.longitude))];
  const av = unit(a), bv = unit(b), v = av.map((x, i) => lerp(x, bv[i]));
  const result: Observation = { time, latitude: degrees(Math.atan2(v[1], Math.hypot(v[0], v[2]))), longitude: degrees(Math.atan2(v[2], v[0])), altitudeMSL: lerp(a.altitudeMSL, b.altitudeMSL), groundSpeed: lerp(a.groundSpeed, b.groundSpeed), track: (angle(a.track, b.track, t) + 360) % 360, velocityAvailable: a.velocityAvailable !== false && b.velocityAvailable !== false };
  for (const key of ['roll', 'pitch', 'heading', 'indicatedAirspeed'] as const) {
    if (a[key] != null && b[key] != null) result[key] = key === 'roll' || key === 'heading' ? angle(a[key], b[key], t) : lerp(a[key], b[key]);
  }
  return result;
}
export function segments(track: Track): Observation[][] {
  const result: Observation[][] = [[]];
  for (const p of track.observations) {
    const last = result.at(-1)!;
    if (last.length && !connected(last.at(-1)!, p)) result.push([]);
    result.at(-1)!.push(p);
  }
  return result.filter(s => s.length > 1);
}
export class Replay {
  track: Track;
  offset = 0;
  rate = 1;
  started: number | undefined;
  constructor(track: Track) { this.track = track; }
  get duration() { return this.track.observations.at(-1)!.time; }
  time(now: number) { return clamp(this.offset + (this.started === undefined ? 0 : Math.max(0, now - this.started) * this.rate), 0, this.duration); }
  playing(now: number) { return this.started !== undefined && this.time(now) < this.duration; }
  pause(now: number) { this.offset = this.time(now); this.started = undefined; }
  play(now: number) { if (this.time(now) >= this.duration) this.offset = 0; this.started = now; }
  seek(time: number, now: number) { if (!Number.isFinite(time)) return; this.offset = clamp(time, 0, this.duration); if (this.started !== undefined) this.started = now; }
  setRate(rate: number, now: number) { if (![1, 4, 16].includes(rate)) return; this.offset = this.time(now); this.rate = rate; if (this.started !== undefined) this.started = now; }
}
