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
  holds the Swift sources and the bundled `style.json` (Dark Matter over OpenFreeMap tiles,
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
xcodegen generate
xcodebuild -project MapLibreVision.xcodeproj -scheme MapLibreVision \
  -destination 'generic/platform=visionOS Simulator' -derivedDataPath build build
xcrun simctl boot "Apple Vision Pro"
xcrun simctl install booted build/Build/Products/Debug-xrsimulator/MapLibreVision.app
xcrun simctl launch --console-pty booted com.sokolysystems.maplibre.vision --enter
```

`--enter` opens the immersive space at launch; without it the launcher window shows a
button. A device build needs the `aarch64-apple-visionos` target in `.cargo/config.toml`
and a signing team in the project.

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

The gestures are visionOS's standard ones, read from the compositor's spatial events. On the
ground a pinch dragged moves the world so that the point the pinch grabbed keeps to the
pinch's ray as the hand turns about the head, however far away it is; on the globe a drag
turns it as if the surface followed the hand. Two pinches pulled apart or together zoom;
two pinches turned about each other turn the world about the viewer, or spin the globe.
Input during a flight is dropped. Two-hand gestures still want a one-hand alternative, as
the Human Interface Guidelines ask.

`MapRenderer` keeps a `MapPlacement`: where the map's scene, metres east, north and up from
the focus on the terrain (`maplibre_visionos_terrain_elevation` reports its elevation),
stands in the room, as a rotation, a position and one uniform scale. Per frame it moves
any flight on, applies the gestures, and passes the pose, with
every eye's pose in the room, the frustum read back from the projection the compositor
computes for the view, the depth range, and the drawable's colour and depth textures, to
`maplibre_visionos_render_frame`. Tiles are requested for a frustum half again as wide as
each eye's, and for the ground all around the eye out to six times its height, so a turn
of the head finds them loaded.

The Rust side turns that into one external view per eye. The map derives its center, zoom
and angles from the eye, which steer the tile covering, and draws with matrices built from
the eye itself: on the globe the center is where the eye's view axis meets the sphere and
the globe camera is the eye; on the flat map the eye's matrix replaces the camera's. It
runs one frame of the schedule per eye, multisampled, and copies the frame's depth into
the drawable's depth texture. The host commits its copies and the presentation on the
map's own Metal queue (`maplibre_visionos_command_queue`), so they run after the map's
draws without a wait on the CPU. When every view owns a whole colour texture that can be
viewed in the map's linear `bgra8Unorm` format the map draws straight into them, both eyes
in one call; otherwise each eye is drawn into the map's own texture and copied into its
texture and slice of the drawable before the next eye is drawn. The compositor then has
colour and depth for every eye. The first frame logs the drawable's layout (`compositor
views`), which says which path the device took.

## Known limits

- wgpu 22 is patched rather than upgraded; a wgpu release with visionOS support (25 and
  later) removes the vendoring.
- The map draws whatever the eye sees; the horizon, the sky and the fog come from the eye's
  own matrices and height, and the style's `sky` sets their colours.
- Each eye runs its own map frame; a tile that arrives between the two is drawn for one eye
  a frame before the other.
