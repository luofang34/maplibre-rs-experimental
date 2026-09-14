import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Vector3 } from 'three';
import { grabPlacement, trackPlacement } from './placement.ts';
import { radians } from './geo.ts';

test('FPV places ownship at the viewing origin with track straight ahead', () => {
  const point = new Vector3(70000, 1500, -30000), origin = new Vector3(0.1, 1.5, 0.2);
  for (const bearing of [0, 90, 180, 248, 359]) {
    const pose = trackPlacement(point, bearing, origin);
    assert.ok(point.clone().applyQuaternion(pose.orientation).add(pose.position).distanceTo(origin) < 1e-8);
    const forward = new Vector3(Math.sin(radians(bearing)), 0, -Math.cos(radians(bearing))).applyQuaternion(pose.orientation);
    assert.ok(forward.distanceTo(new Vector3(0, 0, -1)) < 1e-8);
    assert.ok(new Vector3(0, 1, 0).applyQuaternion(pose.orientation).distanceTo(new Vector3(0, 1, 0)) < 1e-8);
  }
});
test('a one-hand grab follows the hand without a distance-dependent gain', () => {
  const start = { midpoint: new Vector3(0, 1, -1), position: new Vector3(0, 0, -2), separation: 0, scale: 1 / 10000 };
  const current = start.midpoint.clone().add(new Vector3(0.2, 0.1, -0.3));
  const result = grabPlacement(start, current, 0, 10000);
  assert.ok(result.position.clone().sub(start.position).distanceTo(new Vector3(0.2, 0.1, -0.3)) < 1e-8);
  assert.equal(result.scale, start.scale);
});
test('two-hand zoom keeps the picked midpoint fixed including at scale limits', () => {
  const start = { midpoint: new Vector3(0.2, 0.8, -1), position: new Vector3(0, 0, -2), separation: 0.2, scale: 1 / 10000 };
  const anchor = start.midpoint.clone().sub(start.position).divideScalar(start.scale);
  for (const separation of [0.001, 0.4, 10]) {
    const current = new Vector3(0.3, 0.9, -1.1);
    const result = grabPlacement(start, current, separation, 10000);
    assert.ok(anchor.clone().multiplyScalar(result.scale).add(result.position).distanceTo(current) < 1e-8);
    assert.ok(result.scale >= 0.25 / 10000 && result.scale <= 4 / 10000);
  }
});
