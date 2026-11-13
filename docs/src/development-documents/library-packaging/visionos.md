# visionOS

The standalone [`apple/visionos`](../../../../apple/visionos) crate packages `maplibre` as a static C archive (`libmaplibre_visionos.a`) paired with [`apple/visionos/maplibre_visionos.h`](../../../../apple/visionos/maplibre_visionos.h) for Swift and CompositorServices applications on Apple Vision Pro (`MapLibreVision`).

Unlike windowed `winit` targets, visionOS ImmersiveSpace rendering is driven by Apple's `LayerRenderer` frame loop. The host owns the `MTLDevice` and per-eye drawable textures, and hands them across the C ABI each frame with zero CPU readback.

## Building & Checking

The visionOS targets (`aarch64-apple-visionos` and `aarch64-apple-visionos-sim`) are Tier 3 Rust targets and build standard library crates from source via `-Z build-std=std,panic_abort` using the nightly toolchain pinned in `apple/visionos/rust-toolchain.toml`:

```bash
# Typecheck and lint both device and simulator targets (matches CI):
just check-visionos

# Build release static libraries for simulator and device:
cargo +nightly build --manifest-path apple/visionos/Cargo.toml \
    -Z build-std=std,panic_abort --release --target aarch64-apple-visionos-sim
cargo +nightly build --manifest-path apple/visionos/Cargo.toml \
    -Z build-std=std,panic_abort --release --target aarch64-apple-visionos
```

The `MapLibreVision` Xcode project invokes `apple/visionos/MapLibreVision/Scripts/build-rust.sh` before linking to build the matching slice automatically.

## C ABI Overview (`maplibre_visionos.h`)

All functions in `maplibre_visionos.h` catch Rust panics at the FFI boundary, return explicit status codes (`0` on success, negative codes on invalid arguments, initialization failure, or frame error), and document ownership of every pointer.

### Shared Metal Device & Queue

- `maplibre_visionos_new(metal_device, style_json, width, height, sample_count)` / `maplibre_visionos_new_configured(..., config)` wrap the caller's `id<MTLDevice>` inside `wgpu_hal::api::Metal` via `HostGpu` so `maplibre` and the host Swift renderer share one GPU device and pipeline cache without copying textures across APIs.
- `maplibre_visionos_command_queue(map)` returns the unretained `id<MTLCommandQueue>` `wgpu` submits on. Submitting the host's post-processing and `LayerRenderer.Drawable.encodePresent` on this same queue guarantees FIFO execution after map passes without CPU stalls.
- `MaplibreVisionOSConfig` configures `cache_dir` (disk HTTP cache), `bundle_dir` (offline `asset://` tile/glyph/sprite lookup), `color_srgb` (`BGRA8UnormSrgb` for `LayerRenderer`, `BGRA8Unorm` for offscreen/UI), and ` skirts` (`MAPLIBRE_VISIONOS_SKIRTS_NONE` or `AUTO`).

### Stereo `XrFrame` & `ScenePlacement`

`maplibre_visionos_render_stereo` drives `HeadlessMap::render_xr` (`maplibre::render::xr::XrFrame`):

- **`MaplibreVisionOSPlacement`** maps directly to `ScenePlacement`:
  - `anchor`: geographic `lon_deg`, `lat_deg`, and `altitude_m` standing at the origin of the map's local East-North-Up scene frame.
  - `world_from_scene`: column-major `4x4` rigid + uniform-scale transform from scene metres into `LayerRenderer` tracking space. Tabletop globes and room-scale terrain use the exact same renderer; transitioning between them only updates `world_from_scene`.
- **`prefetch`**: optional destination `MaplibreVisionOSPlacement` during an animated camera flight. `PrefetchView` requests destination tiles ahead of arrival without requesting intermediate zoom levels swept through mid-flight.
- **`request_overscan`**: factor (`>= 1.0`) expanding each eye's frustum tangents for tile selection so head rotation finds neighbouring tiles already resident in VRAM.
- **`MaplibreVisionOSEye`** (`eyes[0..eye_count]`):
  - `world_from_eye`: column-major `4x4` eye-to-world pose (`deviceAnchor.originFromAnchorTransform * view.transform`).
  - `left_tan`, `right_tan`, `up_tan`, `down_tan` (positive half-angle tangents from `LayerRenderer.Clock` / `View.tangents`) and `near_m`, `far_m` in world metres.
  - `color_texture`: `id<MTLTexture>` (`bgra8Unorm` or `bgra8Unorm_srgb`, created with `.shaderWrite` / `.renderTarget` and `.pixelFormatView` when `LayerRenderer` supplies a typeless/sRGB pair).
  - `msaa_texture`: optional multisample `id<MTLTexture>` resolved into `color_texture`.
  - `depth_texture`: optional `Depth32Float` `id<MTLTexture>` receiving reverse-Z depth (`1.0` at near, `0.0` at far) for visionOS compositor reprojection and hand occlusion.

### Windowed Orbit Camera & Spatial Raycasting

For volumetric or 2D window scenes alongside an ImmersiveSpace:

- `maplibre_visionos_render_orbit` renders a single colour + optional depth target from `MaplibreVisionOSOrbitCamera` (`center_lon_deg`, `center_lat_deg`, `distance_m`, `pitch_deg`, `bearing_deg`, `fov_y_deg`).
- `maplibre_visionos_screen_to_ground`, `maplibre_visionos_drag_orbit`, and `maplibre_visionos_move_anchor_to_screen` project gaze/pinch rays onto the 3D terrain surface or reference sphere/ellipsoid.

### Terrain, Planetary Bodies & Celestial Environment

- `maplibre_visionos_terrain_elevation(map, lon_deg, lat_deg, &out_meters)` samples loaded `raster-dem` tiles synchronously on the map thread for anchoring spatial pins and labels onto 3D relief.
- `maplibre_visionos_set_body` switches planetary radius, terrain exaggeration, polar cap elevations (`PolarPoleElevations`), and procedural relief presets (Earth, Moon, Mars).
- `maplibre_visionos_set_sky` and `maplibre_visionos_set_stars` feed ECEF sun/moon vectors (`maplibre::celestial::EcefDirection`), twilight bands, eclipse fractions, and stellar catalogues (`maplibre::starfield`) into the atmosphere and starfield passes.
- `maplibre_visionos_register_image_provider` and `maplibre_visionos_invalidate_images` register C callbacks resolving missing `namespace:name` style icons on demand (e.g., SF Symbols rasterized on the Swift side).

## Diagnostics, Memory Budgets & Stability Gate

- **Memory pressure**: `maplibre_visionos_set_available_memory(map, available_bytes)` feeds `os_proc_available_memory()` into `maplibre::render::memory_budget`, tightening tile cache retention before visionOS jetsams the process.
- **Frame timeline tracing**: `maplibre_visionos_trace_enable`, `maplibre_visionos_trace_span`, `maplibre_visionos_trace_presentation`, `maplibre_visionos_trace_device`, and `maplibre_visionos_trace_export` record CPU/GPU stage timings, per-frame upload bytes, `LayerRenderer` presentation intervals, and thermal state to JSON.
- **Automated stability check**: `scripts/check-immersive-stability.py` parses `[immersive-frame]` logs from simulator or device runs and fails if frame p95, consecutive over-budget bursts, memory growth, tile thrash, or camera smoothness regress beyond configured thresholds:

```bash
python3 scripts/check-immersive-stability.py immersive-run.log
```
