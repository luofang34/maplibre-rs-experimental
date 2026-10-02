export const RADIUS = 6378137;
export const WORLD = 2 * Math.PI * RADIUS;
export const radians = (degrees: number) => degrees * Math.PI / 180;
export const degrees = (radians: number) => radians * 180 / Math.PI;
export const clamp = (v: number, low: number, high: number) => Math.max(low, Math.min(high, v));
export const angle = (a: number, b: number, t: number) => a + ((b - a + 540) % 360 - 180) * t;
export function mercator(longitude: number, latitude: number): [number, number] {
  return [(longitude + 180) / 360, (1 - Math.asinh(Math.tan(radians(clamp(latitude, -85.05112878, 85.05112878)))) / Math.PI) / 2];
}
export function geographic(x: number, y: number): [number, number] {
  return [x * 360 - 180, degrees(Math.atan(Math.sinh(Math.PI * (1 - 2 * y))))];
}
export function distance(a: { latitude: number; longitude: number }, b: { latitude: number; longitude: number }) {
  const dl = radians(b.longitude - a.longitude), dp = radians(b.latitude - a.latitude);
  const h = Math.sin(dp / 2) ** 2 + Math.cos(radians(a.latitude)) * Math.cos(radians(b.latitude)) * Math.sin(dl / 2) ** 2;
  return 2 * RADIUS * Math.asin(Math.sqrt(clamp(h, 0, 1)));
}
