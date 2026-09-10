# MapLibre Vision

A visionOS host for the maplibre-rs renderer: the globe with worldwide terrain, drawn into
an immersive space through CompositorServices. The renderer runs as a static library behind
a small C ABI (`maplibre_visionos.h`); the Swift app hands it the head pose and frustum of
each eye and copies the rendered texture into the compositor drawable.

## Layout

- `Cargo.toml`, `src/lib.rs`: the `maplibre-visionos` static library and its C ABI. It is
  its own workspace so the wgpu patch below does not touch the main workspace.
- `vendor-wgpu.sh`: copies wgpu 22, naga, metal-rs and the Objective-C runtime crates from
  the cargo registry into `vendor/` and widens their macOS/iOS gates to visionOS.
  `.cargo/config.toml` patches crates.io with those copies for this crate alone.
- `MapLibreVision/project.yml`: the xcodegen spec of the app. `MapLibreVision/MapLibreVision/`
  holds the Swift sources and the bundled `style.json` (Alpine dusk over OpenFreeMap tiles,
  AWS Terrarium elevation worldwide, globe projection).

## Building for the simulator

Requirements: Xcode with the visionOS platform, `xcodegen`, and the nightly toolchain named
in `rust-toolchain.toml` with `rust-src` (visionOS is a tier 3 target, so the standard
library is built from source).

```sh
cd apple/visionos
./vendor-wgpu.sh
cargo build --release            # target and build-std come from .cargo/config.toml
cd MapLibreVision
sh Scripts/build-flight-overlay.sh aarch64-apple-visionos-sim
xcodegen generate
xcodebuild -project MapLibreVision.xcodeproj -scheme MapLibreVision \
  -destination 'generic/platform=visionOS Simulator' -derivedDataPath build build
xcrun simctl boot "Apple Vision Pro"
xcrun simctl install booted build/Build/Products/Debug-xrsimulator/MapLibreVision.app
xcrun simctl launch --console-pty booted com.sokolysystems.maplibre.vision --enter
```

A fresh launch opens the persistent desk Volume. visionOS restores its placement.
`--replay` opens the selected recording in FPV; `--track mach-loop --replay` opens the
labeled simulation. `--replay-rate 4` accelerates replay. `--enter` opens free exploration.
A device build needs the `aarch64-apple-visionos` target in `.cargo/config.toml`
and a signing team in the project.

## Flight replay

Fly approach starts at the recorded aircraft position. FPV keeps head movement independent
of aircraft motion. A drag detaches into free camera and pauses the recording. Returning
to FPV or Chase eases to the aircraft before resuming. Look forward recenters the viewing
reference. Selecting another recording leaves the free camera in place.

AirDrop, Open With, and Import track accept GPX, timestamped absolute-altitude KML
`gx:Track`, and flight JSON. Imports open in a preview and are stored locally after
confirmation. GPX requires an explicit EGM96 elevation choice; ellipsoid elevations also
require `geoidheight`. Flight JSON uses SI units, true-north angles in degrees, and EGM96
MSL altitude. `indicatedAirspeed`, `roll`, `pitch`, and `heading` are optional. Absent values
stay absent. The importer rejects files over 8 MB or 20,000 samples. The library holds
at most 20 imports. Segment breaks and gaps over 20 seconds are never interpolated.

The transparent `svs-replay` set lives in `MapLibreVision/IndicateOverlay`, independently
of Indicate's G5 set. The host uses pinned Indicate and IndicateAppleDisplay revisions.
Readouts update at most 20 times per second in three reusable buffers; stereo placement
updates with each compositor frame. The overlay reports missing, stale, and failed fields.
GPS ground speed is never substituted for IAS. The FAA AC 20-185A PDF is available from
SVS reference. This replay does not claim certification or operational navigation support.

Run `sh Scripts/check-flight-overlay.sh` from `MapLibreVision` for the Rust overlay gates,
and `swift test` for import, playback, camera, and interaction tests. Build the device
bridge with `sh Scripts/build-flight-overlay.sh aarch64-apple-visionos` before Xcode.
The scripts under `MapLibreVision/Scripts` reproduce both bundled example tracks.

## Viewpoint

The viewer stands at one continuous height above a focus on the surface, from a hundred
and fifty metres over the terrain to forty thousand kilometres, which is the globe on the
table. Below sixty kilometres the world lies level under the viewer at full size; above
it the scene climbs a bridge towards the globe, its origin keeping to the arc between its
two directions from the viewer while distance and scale ease through every magnitude. The
room gives way to the world past the middle of the bridge on the way down, and returns a
little higher on the way up, so a zoom that hovers near the boundary does not flap the
immersion. Two-hand zoom moves the height directly, so zooming into the globe ends on the
street and zooming out from the ground ends at the globe; the launcher's two buttons only
fly there over six seconds. For scripted runs, `--mode tableGlobe` or `--mode immersive`
picks the starting viewpoint, `--height N` starts N metres above the focus, and
`--switch-after N` presses the other button N seconds later, so any point of the bridge can
be captured with `xcrun simctl io booted screenshot`.

- **Table globe**, in mixed immersion: a globe thirty centimetres across a metre in front of
  the viewer, the focus facing them with north upwards, with the room visible around it.
- **Immersive**, in full immersion: the world at full size and level with the room, the
  viewer four kilometres above Innsbruck with the Alps as terrain, the horizon at eye level
  under a night sky, distant ridges fading into haze. The sky is a gradient until a
  celestial layer exists.

A short pinch selects a visible label or icon. Holding and dragging one pinch turns the
globe or pans terrain under the grabbed point. Spread two pinches to zoom, or twist them
to rotate. Moving both hands together carries the table globe. In immersive mode, move
both hands sideways to orbit and vertically to pitch around the center ground target.
Each two-hand interaction locks its intent until a hand releases. Head movement remains
independent of navigation. The Map tilt slider captures one terrain point for the complete
adjustment and reports
when clearance limits the angle. Zoom rebases the geographic frame at its target before
crossing into terrain. Input batches preserve gesture order and cancel excessive backlogs.
Free-camera orbit preserves its target; a fixed-viewpoint policy
preserves the eye for future vehicle-mounted cameras.

See [the interaction design](INTERACTION_DESIGN.md) for gesture ownership, the planned
great-circle ruler, and future airport selection through system-managed gaze and pinch.

`MapRenderer` keeps a `MapPlacement`: where the map's scene, metres east, north and up from
the focus on the terrain (`maplibre_visionos_terrain_elevation` reports its elevation),
stands in the room, as a rotation, a position and one uniform scale. Per frame it moves
any flight on, applies the gestures, and passes the pose, with
every eye's pose in the room, the frustum read back from the projection the compositor
computes for the view, the depth range, and the drawable's colour and depth textures, to
`maplibre_visionos_render_frame`. Tiles are requested for a frustum half again as wide as
each eye's, and for the ground all around the eye out to twice its height, so a turn
of the head finds them loaded.

The Rust side turns that into one external view per eye. The map derives its center, zoom
and angles from the eye, which steer the tile covering, and draws with matrices built from
the eye itself: on the globe the center is where the eye's view axis meets the sphere and
the globe camera is the eye; on the flat map the eye's matrix replaces the camera's. It
ingests tiles once per stereo frame, then renders each eye with its own matrices and the
same terrain refinement, multisampled, and copies the frame's depth into
the drawable's depth texture. The host commits its copies and the presentation on the
map's own Metal queue (`maplibre_visionos_command_queue`), so they run after the map's
draws without a wait on the CPU. When every view owns a whole colour texture that can be
viewed in the map's linear `bgra8Unorm` format the map draws straight into them, both eyes
in one call; otherwise the host keeps a separate linear texture per eye, renders both in one call,
and copies them into the drawable's textures and slices on the map's queue. The compositor then has
colour and depth for every eye. The first frame logs the drawable's layout (`compositor
views`), which says which path the device took.

Visible tiles take priority over surround and flight prefetch. Deferred requests continue
while the head is stationary. Empty geometry does not consume upload slots. Vector uploads
preserve distance priority, and terrain gets request slots alongside vector tiles. The eye
selects detail from physical eye height, distance to each tile and the host's screen resolution.
Sampled gaze elevation and head rotation do not change that metric. Split and merge thresholds
have a 0.15-level margin, shared across the stereo frame, while newly visible ground loads
immediately. Distant tiles coarsen to fit coverage and texture budgets without dropping the
foreground. Terrain keeps a valid ancestor texture or a background surface while its own
texture loads. Undrawn textures stay hidden, including across cache eviction and reuse.

Crossing the geometric horizon uses the same loading reach on both sides, keeping a nearby
eye in the same projection. Immersive fog uses radial distance from the eye in one shared
local frame, so head turns and tile boundaries do not produce rectangular haze edges.

The terrain uses a muted alpine palette and directional slope lighting in immersive views.
Lighting preserves alpha and does not issue additional elevation requests.

## Known limits

- wgpu 22 is patched rather than upgraded; a wgpu release with visionOS support (25 and
  later) removes the vendoring.
- The map draws whatever the eye sees; the horizon, the sky and the fog come from the eye's
  own matrices and height, and the style's `sky` sets their colours.
