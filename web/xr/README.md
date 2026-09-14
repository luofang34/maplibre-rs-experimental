# Map + replay in WebXR

An isolated browser adapter: MapLibre GL JS supplies the globe and map surface; Three.js
draws a continuous elevation mesh, altitude-aware recording, and per-eye WebXR views.
Style and recordings are read directly from the visionOS demo and copied at build time.
This does **not** run the native maplibre-rs renderer in WebXR.

```sh
cd web/xr
npm ci
npm test
npm run build
npm run preview                 # http://localhost:4174
# In another terminal, verify every built asset is served correctly:
npm run verify:local
```

Choose a recording, then **3D replay** (overview) or **Track FPV**. Preparing terrain completes
before **Enter VR** becomes available. Headset controls are rendered in the scene: Play/Pause,
Overview, Track FPV, Exit. Pinch the terrain to place it; use two pinches to scale around
their midpoint. Mouse/touch orbit and zoom work in the browser preview. Exit XR pauses replay.
The globe remains available in the normal browser view; the XR scene contains the regional terrain.

For a headset on the same LAN, a trusted HTTPS origin is required. `localhost` on the headset
means the headset itself, not the development Mac.

```sh
npm run cert
XR_HTTPS=1 npm run preview -- --port 4175
```

Install and explicitly trust **only** `.cert/local-ca.cer` on the test headset, then open
`https://<Mac-LAN-IP>:4175`. The script does not alter any device's trust store. Never share
`*-key.pem`. Regenerate the server certificate if the Mac's address changes. For development,
use `npm run dev` on port 5174 or `XR_HTTPS=1 npm run dev -- --port 5175`.

The preview requests `immersive-vr`, capability-detects support, and handles transient-pointer
input. It needs neither hand tracking nor DOM overlays. Safari on visionOS supports this mode;
ordinary desktop/iPad browsers retain the map and 3D preview when XR is unavailable.

Terrain preparation is bounded to 48 DEM tiles, four fetches at a time, a 257×257 shared grid,
and a 2048×2048 color surface. Complete-region preloading avoids streaming stalls during replay,
but resolution is finite: this is not worldwide, adaptive, near-ground XR terrain. Geographic
zoom and full globe exploration remain in MapLibre's normal browser view. Roads and bridges
are draped cartography; XR place labels and instruments are outside this prototype.

Replay keeps missing fields absent and does not interpolate gaps over 20 seconds or segment
breaks. Innsbruck receiver coverage ends before the runway; its approach identification is
unconfirmed. Mach Loop is explicitly simulated. Track FPV follows the ground track with a
level reference; it does not invent aircraft attitude. Map-view route lines are draped;
the spatial route uses recorded MSL heights over EGM96 terrain.

Platform policy: **iPad = SVS, no HWD; visionOS = HWD by default.** Instrument rendering belongs
in Indicate and remains independent of this map/replay adapter.

References: [WebKit WebXR input](https://webkit.org/blog/15162/introducing-natural-input-for-webxr-in-apple-vision-pro/),
[Safari WebXR/WebGPU](https://webkit.org/blog/17640/webkit-features-for-safari-26-2/),
[Three.js XR](https://threejs.org/docs/pages/WebXRManager.html),
[MapLibre terrain](https://maplibre.org/maplibre-gl-js/docs/examples/3d-terrain/).
