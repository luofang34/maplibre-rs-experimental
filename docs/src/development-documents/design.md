# Design Principles & Trade-offs

This document records the core engineering decisions behind `maplibre` and how they shape day-to-day development across `maplibre`, `maplibre-winit`, `web`, and `apple/visionos`.

## 1. Domain-Specific TCS Over Generic ECS

Map rendering has a very specific data topology:

- Global singleton state (`Style`, `ViewState`, `WgpuTileViewPattern`, `GpuDemPool`, atlases, frame timings).
- Sparse, hierarchical per-tile components keyed by `(z, x, y, build_id)` (`WorldTileCoords`) that are queried along parent/child/ancestor pyramid paths every frame (for overscaled vector fallbacks, DEM ancestor draping, and stencil masking).

Using `maplibre::tcs` (`World`, `Tiles`, `Resources`) instead of an external archetype ECS gives:

1. **Direct pyramid traversal** (`tcs::pyramid`) without entity-ID hash lookups when resolving ancestor fallback tiles.
2. **Zero-macro system signatures**: systems are plain Rust functions or structs implementing `System::run(&mut self, &mut MapContext) -> Result<(), SystemError>`.
3. **Lightweight WebAssembly footprint**: avoids procedural-macro and archetype-table bloat in `wasm32-unknown-unknown` bundles.

## 2. Explicit `Schedule` Stages and Fallible Lifecycle

GPU applications in browsers, mobile apps, and XR headsets routinely face recoverable environment failures: lost GPU surfaces, suspended windows, invalid user styles, or exhausted memory budgets.

- **No non-test panics**: production crates enforce `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` alongside `scripts/check-panics.mjs` in CI.
- **Fallible `Plugin::build` and `Stage::run`**: plugin installation returns `Result<(), PluginError>` and frame execution returns `StageResult`.
- **Guaranteed frame cleanup (`Schedule::run_after_failure`)**: `RenderStageLabel::Cleanup` runs even when an earlier stage errors, ensuring transient per-frame `RenderPhase` vectors and tile retention budgets never leak across frames.

## 3. Worker Offload via `Environment` and `Transferables`

To hold `60–90+ FPS` (especially on Apple Vision Pro where missed CompositorServices deadlines cause visible judder), the render thread performs **no** network I/O, protobuf/MLT parsing, polygon tessellation, or font glyph rasterization:

- `Environment` parameterizes `Kernel<E>` over `MapWindowConfig`, `AsyncProcedureCall`, `Scheduler`, and `OffscreenKernelEnvironment`.
- `VectorTransferables`, `RasterTransferables`, and `DemTransferables` let native builds pass heap-allocated buffers directly across channels while single-threaded Web Worker builds serialize into FlatBuffers transferred via zero-copy `postMessage` transfer lists.
- `SharedLoader` (`ResourceLoader`) centralizes caching and in-flight request coalescing so simultaneous tile/glyph/sprite requests from multiple workers share one HTTP connection pool and disk cache.

## 4. In-Place GPU Updates & Byte-Counted Uploads

Re-tessellating an entire vector tile whenever a hover state or theme changes wastes CPU and frame upload bandwidth. Instead:

1. **Expression classification** (`maplibre::style::expression`): every parsed Style Spec expression statically computes `output_type()`, `is_constant()`, `is_feature_constant()`, and `is_zoom_constant()`.
2. **Targeted mutation classification** (`maplibre::style::mutation`):
   - Uniform-only paint changes update GPU uniform bindings in `Prepare`.
   - Feature-dependent paint, `feature-state`, or `global-state` changes re-evaluate feature paint columns (`vector::feature_paint`) against retained feature properties and write updated byte ranges into `BufferPool` via `UploadQueue::write_buffer`.
   - Only layout or structural source changes trigger worker re-processing.
3. **Strict upload accounting**: `UploadQueue` wraps `wgpu::Queue` and counts every uploaded byte per frame. `clippy.toml` forbids calling raw `wgpu::Queue::write_buffer` / `write_texture` outside `UploadQueue`, preventing hidden upload spikes.

## 5. Unified 2D, Globe, 3D Terrain, and Stereo XR Pipeline

Rather than maintaining separate renderers for 2D Mercator, 3D globes, terrain, and headsets:

- **Coordinate & projection separation**: tile workers always tessellate into normalized tile-local coordinates (`0..EXTENT`). Vertex shaders project tile vertices into either flat Web Mercator, curved ECEF `Globe` coordinates, or displaced 3D `raster-dem` meshes, transitioning continuously across zoom levels.
- **Hybrid terrain draping**: flat fill/line/raster layers drape onto 3D terrain via cached render-to-texture (`DrapePassNode` + `DrapeBudget`), while 3D extrusions (`fill-extrusion`) and screen-aligned `symbol` labels render directly in the main pass at sampled terrain elevations so vertical walls and crisp text never blur on steep slopes.
- **View abstraction (`ResolvedFrameView` & `XrFrame`)**: a standard window renders a single `ViewState`; `MultiViewFrame` iterates over N named `ViewSlotId` cameras; and `XrFrame` renders `N` eyes (`XrEye`) from a shared `ScenePlacement` while computing tile requests once across the combined stereo frustum.

## 6. Ratcheted Conformance & Static Architecture Guards

Correctness and architectural hygiene are enforced continuously in CI:

- **MapLibre Style Spec Conformance**: `render-tests` runs 1,700+ pixel (`expected.png`) and query (`expected.json`) fixtures on software Vulkan (`lavapipe`) with a ratcheted `render-tests/ignores.json` baseline (any unexpected failure *or* unremoved newly-passing ignore fails CI), complemented by `maplibre/tests/expression_conformance.rs`.
- **Static guards**: `scripts/check-file-length.mjs` (`<= 500` lines per `.rs` file), `scripts/check-module-layout.py` (no `mod.rs`), `scripts/check-panics.mjs`, `scripts/check-wasm-deps.py` (forbids native OS/runtime/TLS crates in `wasm32-unknown-unknown` trees), and `cargo-deny` (`deny.toml`) keep crate boundaries clean as the engine grows.
