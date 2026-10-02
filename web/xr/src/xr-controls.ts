import * as THREE from 'three';

export type Action = 'play' | 'overview' | 'fpv' | 'exit';
export class SpatialControls {
  canvas = document.createElement('canvas');
  texture: THREE.CanvasTexture;
  mesh: THREE.Mesh<THREE.PlaneGeometry, THREE.MeshBasicMaterial>;
  private last = '';
  constructor() {
    this.canvas.width = 1024; this.canvas.height = 240;
    this.texture = new THREE.CanvasTexture(this.canvas); this.texture.colorSpace = THREE.SRGBColorSpace;
    this.mesh = new THREE.Mesh(new THREE.PlaneGeometry(1.1, 1.1 * 240 / 1024), new THREE.MeshBasicMaterial({ map: this.texture, transparent: true, depthTest: false }));
    this.mesh.renderOrder = 100;
    this.mesh.position.set(0, -0.55, -1.5);
    this.mesh.rotation.x = -0.18;
  }
  update(title: string, playing: boolean, time: string) {
    const key = `${title}|${playing}|${time}`; if (key === this.last) return; this.last = key;
    const c = this.canvas.getContext('2d')!;
    c.clearRect(0, 0, 1024, 240); c.fillStyle = '#132722ee'; c.beginPath(); c.roundRect(0, 0, 1024, 240, 28); c.fill();
    c.font = '32px system-ui'; c.fillStyle = '#f2f7ed'; c.fillText(title.slice(0, 42), 30, 52);
    c.font = '24px system-ui'; c.fillStyle = '#c2d7cb'; c.fillText(time, 30, 92);
    [playing ? 'Pause' : 'Play', 'Overview', 'Track FPV', 'Exit XR'].forEach((label, i) => {
      c.fillStyle = '#2e5248'; c.beginPath(); c.roundRect(22 + i * 250, 122, 228, 90, 18); c.fill();
      c.fillStyle = '#ffffff'; c.font = '28px system-ui'; c.textAlign = 'center'; c.fillText(label, 136 + i * 250, 178); c.textAlign = 'left';
    });
    this.texture.needsUpdate = true;
  }
  pick(ray: THREE.Raycaster): Action | undefined {
    const hit = ray.intersectObject(this.mesh)[0];
    if (!hit?.uv || hit.uv.y > 0.5 || hit.uv.y < 0.1) return;
    return (['play', 'overview', 'fpv', 'exit'] as const)[Math.min(3, Math.floor(hit.uv.x * 4))];
  }
  dispose() { this.mesh.geometry.dispose(); this.mesh.material.dispose(); this.texture.dispose(); this.canvas.width = this.canvas.height = 1; }
}
export function inputRay(frame: XRFrame, source: XRInputSource, reference: XRReferenceSpace): THREE.Raycaster | undefined {
  const pose = frame.getPose(source.targetRaySpace, reference); if (!pose) return;
  const matrix = new THREE.Matrix4().fromArray(pose.transform.matrix);
  return new THREE.Raycaster(new THREE.Vector3().setFromMatrixPosition(matrix), new THREE.Vector3(0, 0, -1).transformDirection(matrix));
}
