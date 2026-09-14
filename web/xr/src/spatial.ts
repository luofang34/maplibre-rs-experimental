import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { Line2 } from 'three/addons/lines/Line2.js';
import { LineGeometry } from 'three/addons/lines/LineGeometry.js';
import { LineMaterial } from 'three/addons/lines/LineMaterial.js';
import { radians } from './geo.ts';
import { grabPlacement, trackPlacement, type Grab } from './placement.ts';
import { segments, sample, type Replay } from './replay.ts';
import { SpatialControls, inputRay, type Action } from './xr-controls.ts';
import type { Terrain } from './terrain.ts';

export class SpatialView {
  renderer: THREE.WebGLRenderer;
  scene = new THREE.Scene();
  root = new THREE.Group();
  camera = new THREE.PerspectiveCamera(60, innerWidth / innerHeight, 0.03, 300000);
  orbit: OrbitControls;
  panel = new SpatialControls();
  mode: 'overview' | 'fpv' = 'overview';
  ownship: THREE.Mesh;
  lines: Line2[] = [];
  private replay: Replay;
  private terrain: Terrain;
  private session?: XRSession;
  private entering = false;
  private disposed = false;
  private followOrigin = new THREE.Vector3();
  private resetOrigin = true;
  private active = new Map<XRInputSource, THREE.Vector3>();
  private drag?: Grab;
  private observer: ResizeObserver;
  onAction: (action: Action) => void;
  onExit: () => void = () => {};

  constructor(container: HTMLElement, terrain: Terrain, replay: Replay, action: (action: Action) => void) {
    this.terrain = terrain; this.replay = replay; this.onAction = action;
    this.renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, powerPreference: 'high-performance' });
    this.renderer.setPixelRatio(Math.min(devicePixelRatio, 1.5));
    this.renderer.xr.enabled = true;
    this.renderer.xr.setReferenceSpaceType('local');
    this.renderer.xr.setFramebufferScaleFactor(0.85);
    this.renderer.xr.setFoveation(0.6);
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    container.append(this.renderer.domElement);
    this.scene.background = new THREE.Color('#afc7ce');
    this.scene.add(new THREE.HemisphereLight('#e4f1ff', '#5b6150', 2.1));
    const sun = new THREE.DirectionalLight('#fff4dd', 2.1); sun.position.set(-1, 2, 1); this.scene.add(sun);
    this.root.add(terrain.mesh); this.scene.add(this.root); this.scene.add(this.panel.mesh);
    this.panel.mesh.visible = false;
    const material = new LineMaterial({ color: '#00bcb9', linewidth: 3, alphaToCoverage: true, depthTest: true });
    for (const segment of segments(replay.track)) {
      const geometry = new LineGeometry(); geometry.setPositions(segment.flatMap(p => terrain.toLocal(p).toArray()));
      const line = new Line2(geometry, material); this.lines.push(line); this.root.add(line);
    }
    const plane = new THREE.ConeGeometry(1, 3, 3); plane.rotateX(-Math.PI / 2);
    this.ownship = new THREE.Mesh(plane, new THREE.MeshBasicMaterial({ color: '#fff5c6' })); this.root.add(this.ownship);
    this.orbit = new OrbitControls(this.camera, this.renderer.domElement); this.orbit.enableDamping = true; this.orbit.maxPolarAngle = Math.PI * 0.48;
    this.orbit.minDistance = 0.2; this.orbit.maxDistance = 8;
    this.overview();
    this.observer = new ResizeObserver(() => { this.renderer.setSize(container.clientWidth, container.clientHeight); this.camera.aspect = container.clientWidth / container.clientHeight; this.camera.updateProjectionMatrix(); });
    this.observer.observe(container);
  }
  overview() {
    this.camera.near = 0.03; this.camera.far = 50; this.camera.updateProjectionMatrix();
    this.mode = 'overview'; this.orbit.enabled = !this.session; this.scene.fog = null;
    this.root.rotation.set(0, 0, 0); this.root.scale.setScalar(1.6 / this.terrain.width);
    this.root.position.set(0, this.session ? -0.5 : 0, this.session ? -1.6 : 0);
    if (!this.session) { this.camera.position.set(0.8, 1.4, 1.3); this.orbit.target.set(0, 0, 0); this.orbit.update(); }
    this.resetOrigin = true;
  }
  fpv() { this.mode = 'fpv'; this.camera.near = 0.5; this.camera.far = 200000; this.camera.updateProjectionMatrix(); this.orbit.enabled = false; this.root.scale.setScalar(1); this.scene.fog = new THREE.Fog('#afc7ce', 25000, 110000); this.resetOrigin = true; }
  render(now: number, frame?: XRFrame) {
    const time = this.replay.time(now), p = sample(this.replay.track, time);
    this.ownship.visible = !!p && this.mode === 'overview';
    if (p) {
      const position = this.terrain.toLocal(p);
      this.ownship.position.copy(position); this.ownship.rotation.y = -radians(p.track);
      this.ownship.scale.setScalar(this.terrain.width * 0.005);
      if (this.mode === 'fpv') {
        if (this.resetOrigin) {
          const ref = this.renderer.xr.getReferenceSpace();
          const pose = frame && ref ? frame.getViewerPose(ref) : undefined;
          if (!this.session || pose) { this.followOrigin.set(pose?.transform.position.x ?? 0, pose?.transform.position.y ?? 0, pose?.transform.position.z ?? 0); this.resetOrigin = false; }
        }
        const pose = trackPlacement(position, p.track, this.followOrigin);
        this.root.quaternion.copy(pose.orientation); this.root.position.copy(pose.position);
        if (!this.session) { this.camera.position.set(0, 0, 0); this.camera.rotation.set(0, 0, 0); }
      }
    }
    this.updateGesture(frame);
    if (this.mode === 'overview' && !this.session) this.orbit.update();
    this.panel.update(this.replay.track.title ?? this.replay.track.callsign, this.replay.playing(now), p ? `${Math.floor(time)} s  ·  ${Math.round(p.altitudeMSL * 3.28084)} ft MSL  ·  ${Math.round(p.groundSpeed * 1.94384)} kt GS` : 'Recording gap · position unavailable');
    this.renderer.render(this.scene, this.camera);
  }
  async enterXR() {
    if (this.session || this.entering || this.disposed) return;
    this.entering = true;
    try { await this.startXR(); }
    finally { this.entering = false; }
  }
  private async startXR() {
    if (!navigator.xr) throw new Error('This browser does not provide WebXR.');
    const session = await navigator.xr.requestSession('immersive-vr', { optionalFeatures: ['local-floor'] });
    if (this.disposed) { await session.end(); return; }
    this.session = session;
    const selectStart = (event: XRInputSourceEvent) => {
      const ref = this.renderer.xr.getReferenceSpace(); if (!ref) return;
      const ray = inputRay(event.frame, event.inputSource, ref); if (!ray) return;
      const action = this.panel.pick(ray);
      if (action) { this.onAction(action); return; }
      if (this.mode !== 'overview' || !ray.intersectObject(this.terrain.mesh).length) return;
      const pose = event.inputSource.gripSpace && event.frame.getPose(event.inputSource.gripSpace, ref);
      if (pose) this.active.set(event.inputSource, new THREE.Vector3(pose.transform.position.x, pose.transform.position.y, pose.transform.position.z));
      this.drag = undefined;
    };
    const selectEnd = (event: XRInputSourceEvent) => { this.active.delete(event.inputSource); this.drag = undefined; };
    session.addEventListener('selectstart', selectStart);
    session.addEventListener('selectend', selectEnd);
    session.addEventListener('visibilitychange', () => { if (session.visibilityState !== 'visible') this.replay.pause(performance.now() / 1000); });
    session.addEventListener('end', () => {
      session.removeEventListener('selectstart', selectStart); session.removeEventListener('selectend', selectEnd);
      this.session = undefined; this.panel.mesh.visible = false; this.active.clear(); this.drag = undefined;
      this.replay.pause(performance.now() / 1000);
      if (!this.disposed) { this.overview(); this.onExit(); }
    }, { once: true });
    try {
      await this.renderer.xr.setSession(session);
      this.panel.mesh.visible = true;
      this.mode === 'fpv' ? this.fpv() : this.overview();
    } catch (error) { await session.end(); throw error; }
  }
  async exitXR() { await this.session?.end(); }
  private updateGesture(frame?: XRFrame) {
    const ref = this.renderer.xr.getReferenceSpace(); if (!frame || !ref || !this.active.size || this.mode !== 'overview') return;
    for (const source of this.active.keys()) {
      const pose = source.gripSpace && frame.getPose(source.gripSpace, ref);
      if (pose) this.active.get(source)!.set(pose.transform.position.x, pose.transform.position.y, pose.transform.position.z);
    }
    const points = [...this.active.values()].slice(0, 2), midpoint = points.reduce((a, p) => a.add(p), new THREE.Vector3()).divideScalar(points.length);
    const separation = points.length === 2 ? points[0].distanceTo(points[1]) : 0;
    if (!this.drag) this.drag = { midpoint: midpoint.clone(), separation, position: this.root.position.clone(), scale: this.root.scale.x };
    const placement = grabPlacement(this.drag, midpoint, separation, this.terrain.width);
    this.root.position.copy(placement.position); this.root.scale.setScalar(placement.scale);
  }
  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    this.session?.end().catch(error => console.warn('Could not end XR session while closing the view:', error));
    this.renderer.setAnimationLoop(null); this.observer.disconnect(); this.orbit.dispose(); this.panel.dispose();
    for (const line of this.lines) line.geometry.dispose(); this.lines[0]?.material.dispose();
    this.ownship.geometry.dispose(); (this.ownship.material as THREE.Material).dispose();
    this.terrain.dispose(); this.renderer.dispose(); this.renderer.forceContextLoss(); this.renderer.domElement.remove();
  }
}
