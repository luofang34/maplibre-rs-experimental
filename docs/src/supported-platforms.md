# Supported Platforms

`maplibre` runs across desktop, browser, mobile, spatial/XR, and headless server environments on top of `wgpu`.

## Platform Matrix

| Platform | Crate / Target | Graphics Backend | Async & Worker Model | CI Status |
| --- | --- | --- | --- | --- |
| **Linux** | `maplibre-winit`, `maplibre-demo` (`x86_64` / `aarch64`) | Vulkan, OpenGL ES (`wgpu`) | Multi-threaded Tokio + `reqwest` (`SharedLoader`) | Tested on every PR (Vulkan + validation layers + Lavapipe conformance) |
| **macOS** | `maplibre-winit`, `maplibre-demo`, `apple` (`aarch64` / `x86_64`) | Metal | Multi-threaded Tokio + `reqwest` (`SharedLoader`) | Tested on every PR (`macos-15` with Metal shader & debug validation) |
| **Windows** | `maplibre-winit`, `maplibre-demo` (`x86_64`) | DirectX 12, Vulkan | Multi-threaded Tokio + `reqwest` (`SharedLoader`) | Built and tested in CI |
| **Web (Wasm)** | `web` (`wasm32-unknown-unknown`) | WebGPU (default) and WebGL2 (`--features web-webgl`) | Browser `fetch` + dedicated Web Workers (`FlatBufferTransferable` or shared-memory atomics) | Built, dependency-audited (`check-wasm-deps.py`), and tested in headless browsers |
| **visionOS** | `apple/visionos` (`aarch64-apple-visionos`, `aarch64-apple-visionos-sim`) | Shared `MTLDevice` / `MTLCommandQueue` + CompositorServices stereo `MTLTexture` targets | Tokio worker pool + disk/bundle `SharedLoader` | Checked & clippy-gated on `macos-15` (`build-visionos`) |
| **iOS** | `apple` (`aarch64-apple-ios`, `aarch64-apple-ios-sim`) | Metal (`.xcframework`) | Multi-threaded Tokio + `reqwest` | Built in `library-apple` workflow |
| **Android** | `android` (`aarch64-linux-android`, `x86_64-linux-android`, `armv7-linux-androideabi`, `i686-linux-android`) | Vulkan, OpenGL ES (`ndk` / JNI) | Multi-threaded Tokio + `reqwest` | Built in Android packaging workflow |
| **Headless / Server** | `maplibre` (`--features headless`) | Offscreen Vulkan (including Mesa Lavapipe software rasterizer), Metal, DX12, or shared `HostGpu` | Deterministic settle loop + optional `SharedMapAssets` across worlds | 6-shard unit + integration suite and 1,701-case MapLibre render/query conformance gate |

## Feature Flags (`maplibre`)

The core `maplibre` crate keeps platform I/O optional so embedded, WebAssembly, and custom-loader hosts never pull native networking or OS threads unless requested:

- `native-io` *(enabled by default)* — enables Tokio, `reqwest`, `reqwest-middleware`, and HTTP disk caching (`http-cache-reqwest`). Disabling default features (`--no-default-features`) compiles `maplibre` cleanly without native socket or TLS crates.
- `headless` — enables `maplibre::headless` (`HeadlessMap`, `HostGpu`, `SharedMapAssets`, `MultiViewFrame`, and PNG readback). Works both with `native-io` and with `--no-default-features --features headless` using a custom `ResourceLoader`.
- `web-webgl` — configures `wgpu` limits and downlevel compatibility for WebGL2 contexts on `wasm32-unknown-unknown`.
- `thread-safe-futures` — requires `Send` bounds on `Scheduler` and `HttpClient` futures for multi-threaded native hosts (omitted on single-threaded `wasm32`).
- `mlt` *(enabled by default)* — decodes MapLibre Tiles (`.mlt`) alongside classic Mapbox Vector Tiles (`.mvt` / `.pbf`).
- `pmtiles` — supports HTTP range-request reading from single-file `.pmtiles` archives.
- `trace` — emits `tracing` spans per schedule stage and render pass for profiling.
