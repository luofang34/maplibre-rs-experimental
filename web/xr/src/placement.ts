import { Vector3, Quaternion } from 'three';
import { clamp, radians } from './geo.ts';

export interface Grab {
  midpoint: Vector3; separation: number; position: Vector3; scale: number;
}
export function grabPlacement(start: Grab, midpoint: Vector3, separation: number, width: number) {
  const ratio = start.separation > 0.03 ? clamp(separation / start.separation, 0.25, 4) : 1;
  const scale = clamp(start.scale * ratio, 0.25 / width, 4 / width);
  return { scale, position: start.position.clone().sub(start.midpoint).multiplyScalar(scale / start.scale).add(midpoint) };
}
export function trackPlacement(point: Vector3, bearing: number, eyeOrigin: Vector3) {
  const orientation = new Quaternion().setFromAxisAngle(new Vector3(0, 1, 0), radians(bearing));
  return { orientation, position: point.clone().applyQuaternion(orientation).negate().add(eyeOrigin) };
}
