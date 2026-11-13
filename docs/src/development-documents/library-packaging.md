# Library Packaging

`maplibre` separates its platform-agnostic rendering and style engine from platform bindings so each target packages native idioms cleanly:

- **[Apple (iOS & macOS)](./library-packaging/apple.md)** — Static archive and `.xcframework` integration for UIKit and AppKit hosts.
- **[visionOS](./library-packaging/visionos.md)** — Zero-copy Metal + CompositorServices stereo C ABI (`apple/visionos`) for ImmersiveSpace and windowed 3D orbit rendering on Apple Vision Pro.
- **[Android](./library-packaging/android.md)** — JNI / NDK native activity and AAR packaging.
- **[Web](./library-packaging/web.md)** — `cargo` + `wasm-bindgen` + `wasm-opt` + `esbuild` package (`web/lib/build.mjs`) emitting ESM, CJS, and IIFE bundles for WebGPU, WebGL2, and multithreaded Web Workers.
