import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { Replay, decodeTrack, sample, segments, type Track } from './replay.ts';
import { mercator, geographic } from './geo.ts';

const fixture = (): Track => ({ callsign: 'TEST', source: { url: 'https://example.com', license: 'test', coverage: 'Test samples' }, observations: [
  { time: 0, latitude: 47, longitude: 11, altitudeMSL: 1000, groundSpeed: 50, track: 359 },
  { time: 10, latitude: 47, longitude: 11.005, altitudeMSL: 1100, groundSpeed: 50, track: 1 },
] });
test('sample interpolates north crossing without inventing IAS or attitude', () => {
  const p = sample(fixture(), 5)!;
  assert.ok(p.track < 0.001 || p.track > 359.999);
  assert.equal(p.altitudeMSL, 1050); assert.equal(p.indicatedAirspeed, undefined); assert.equal(p.pitch, undefined);
});
test('outages and segment breaks suppress positions and connecting lines', () => {
  const track = fixture(); track.observations[1].time = 30;
  assert.equal(sample(track, 15), undefined); assert.equal(segments(track).length, 0);
  track.observations[1].time = 10; track.observations[1].startsSegment = true;
  assert.equal(sample(track, 5), undefined); assert.equal(segments(track).length, 0);
  assert.equal(sample(track, 10)?.time, 10);
});
test('clock preserves time on speed change, pause, seek and end', () => {
  const replay = new Replay(fixture()); replay.play(100);
  assert.equal(replay.time(102), 2); replay.setRate(4, 102); assert.equal(replay.time(103), 6);
  replay.pause(103); assert.equal(replay.time(200), 6);
  replay.seek(8, 201); replay.play(202); assert.equal(replay.time(203), 10); assert.equal(replay.playing(203), false);
  replay.play(204); assert.equal(replay.time(204), 0);
});
test('decoder rejects malformed, unordered and impossible observations', () => {
  for (const mutate of [(t: Track) => t.observations[1].time = 0, (t: Track) => t.observations[1].latitude = 90, (t: Track) => t.observations[1].longitude = -40, (t: Track) => t.observations[1].pitch = 100]) {
    const track = fixture(); mutate(track); assert.throws(() => decodeTrack(JSON.stringify(track)));
  }
  assert.throws(() => decodeTrack('null'));
});
test('shared native recordings validate without schema translation', () => {
  for (const name of ['innsbruck-approach', 'mach-loop']) {
    const source = new URL(`../../../apple/visionos/MapLibreVision/MapLibreVision/Resources/${name}.json`, import.meta.url);
    const track = decodeTrack(readFileSync(source, 'utf8'));
    assert.ok(track.observations.length > 10); assert.ok(sample(track, 0)); assert.ok(segments(track).length);
  }
});
test('Mercator footprint preserves coordinate registration', () => {
  for (const [lon, lat] of [[11.35, 47.25], [-3.7, 52.7], [179, -40]]) {
    const [x, y] = mercator(lon, lat), result = geographic(x, y);
    assert.ok(Math.abs(result[0] - lon) < 1e-8); assert.ok(Math.abs(result[1] - lat) < 1e-8);
  }
});
