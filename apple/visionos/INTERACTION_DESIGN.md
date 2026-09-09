# Interaction contract

The executable navigation rules live in `MapGestureRecognizer`, `MapNavigation`,
`MapOrbit` and their Swift tests. This file keeps only cross-platform decisions and
integration boundaries.

| Input | Persistent desk Volume | Terrain / expanded map |
| --- | --- | --- |
| One held pinch | Native object rotation | Pan the grabbed geographic point |
| Two hands spreading | Physical size, bounded to 20–44 cm diameter | Geographic zoom at the captured focus |
| Two hands twisting | Native object rotation | Orbit the terrain focus |
| Move the system window handle | Place, snap and lock the whole Volume | Move the control panel |
| Common two-hand movement | Rotation and scale stay independent of placement | Horizontal orbit; vertical tilt |
| Tilt slider | Not shown | Hold one terrain focus until editing ends |

Recognition locks one intent until release. Hand-count changes rebase input;
tracking loss cancels it. Head motion changes the physical view without steering a
captured gesture. Ray motion uses a fixed reference depth; projected zoom scale
follows the hand separation ratio. Terrain and geographic coordinates survive tile
replacement and the globe-to-plane transition. Clearance can limit a requested tilt.

A fresh launch opens a restorable desk Volume. visionOS owns its room placement;
orientation, bounded physical size and paused replay position are saved separately.
The Volume stays open with its contents hidden during terrain exploration. Returning
to the desk reveals it in place. Detailed map controls open beside the Volume.
The native globe uses a bounded, MapLibre-rendered Natural Earth overview; detailed
terrain remains in the Metal compositor. Physical size does not change geographic zoom.

The bundled AUA10A approach contains observed ADS-B positions with GNSS altitude
converted to EGM96 mean sea level. Playback shares one monotonic clock across views,
ends at receiver coverage, and does not invent touchdown or aircraft attitude. The
follow camera stays behind and above the aircraft with independent head tracking;
map gestures detach it. Recorded positions ahead of playback feed bounded prefetch.
`--replay --replay-rate 16` exercises accelerated playback in a debug build.

## Integration boundaries

- Aircraft follow orbits a moving entity in a gravity-aligned frame. Onboard viewing
  uses an aircraft mount, deliberate look offset, then tracked head. It must preserve
  actual aircraft attitude and never apply free-camera clearance to telemetry.
- Attachment transitions preserve the starting world pose, evaluate moving destinations
  at presentation time and preload their visible corridor. Returning to a saved map is
  separate from detaching into free exploration at the current position.
- Keep headset compositor projection; magnification can use a separate instrument panel.
- The Metal `CompositorLayer` is immersive-space content, not a RealityKit entity.
  Terrain entry retains the desk's geographic focus. Its room transform is not a
  substitute for RealityKit coordinate-space conversion during a spatial handoff.
- Reserve stationary two-finger hold for a ruler. Store endpoints geographically;
  compute great-circle distance in metres and display NM. Airport selection uses stable
  feature IDs and confirmed system selection input, not continuous raw eye tracking.
- iPad/web map pitch uses the host camera convention. visionOS Map tilt measures the
  scene's local up relative to room gravity; zero is not an instruction to level the head.

References: [MapLibre camera options](https://maplibre.org/maplibre-gl-js/docs/API/type-aliases/CameraOptions/),
[Apple Metal with passthrough](https://developer.apple.com/videos/play/wwdc2024/10092/),
[Apple persistent UI](https://developer.apple.com/documentation/visionos/adopting-best-practices-for-scene-restoration),
[RealityKit coordinate transfer](https://developer.apple.com/documentation/realitykit/transforming-entities-between-realitykit-coordinate-spaces).
