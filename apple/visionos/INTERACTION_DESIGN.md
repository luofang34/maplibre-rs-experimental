# Interaction contract

The executable navigation rules live in `MapGestureRecognizer`, `MapNavigation`,
`MapOrbit` and their Swift tests. This file keeps only cross-platform decisions and
integration boundaries.

| Input | Table object | Terrain exploration |
| --- | --- | --- |
| Short pinch / tap / click | Select | Select |
| One held pinch / primary drag | Turn the grabbed geographic point | Pan the grabbed geographic point |
| Two hands spreading / pinch / wheel | Geographic zoom at the captured focus | Same |
| Two hands twisting | Rotate the object about its center | Orbit the terrain focus |
| Common two-hand movement | Place the globe in the room | Horizontal orbit; vertical tilt |
| Tilt slider | Not shown | Hold one terrain focus until editing ends |

Recognition locks one intent until release. Hand-count changes rebase input;
tracking loss cancels it. Head motion changes the physical view without steering a
captured gesture. Ray motion uses a fixed reference depth; projected zoom scale
follows the hand separation ratio. Terrain and geographic coordinates survive tile
replacement and the globe-to-plane transition. Clearance can limit a requested tilt.

A fresh launch opens a globe at the initial viewing center. The controls use system
utility-panel placement. Layer recreation retains the scene pose. `--menu-only`
opens just the control window for diagnostics.

## Integration boundaries

- Aircraft follow orbits a moving entity in a gravity-aligned frame. Onboard viewing
  uses an aircraft mount, deliberate look offset, then tracked head. It must preserve
  actual aircraft attitude and never apply free-camera clearance to telemetry.
- Attachment transitions preserve the starting world pose, evaluate moving destinations
  at presentation time and preload their visible corridor. Returning to a saved map is
  separate from detaching into free exploration at the current position.
- Keep headset compositor projection; magnification can use a separate instrument panel.
- A persistent home Volume needs a RealityKit rendering adapter. The current Metal
  `CompositorLayer` is immersive-space content, not a RealityKit entity. Do not treat
  its room coordinates as system-restored Volume placement. Retain geographic state
  independently of physical placement; restore placement through the system's scene APIs.
- Reserve stationary two-finger hold for a ruler. Store endpoints geographically;
  compute great-circle distance in metres and display NM. Airport selection uses stable
  feature IDs and confirmed system selection input, not continuous raw eye tracking.
- iPad/web map pitch uses the host camera convention. visionOS Map tilt measures the
  scene's local up relative to room gravity; zero is not an instruction to level the head.

References: [MapLibre camera options](https://maplibre.org/maplibre-gl-js/docs/API/type-aliases/CameraOptions/),
[Apple Metal with passthrough](https://developer.apple.com/videos/play/wwdc2024/10092/),
[Apple persistent UI](https://developer.apple.com/documentation/visionos/adopting-best-practices-for-scene-restoration),
[RealityKit coordinate transfer](https://developer.apple.com/documentation/realitykit/transforming-entities-between-realitykit-coordinate-spaces).
