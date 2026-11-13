# Web

The browser distribution lives in `web/` (the Rust `wasm32-unknown-unknown` cdylib/rlib crate) and `web/lib/` (the TypeScript host wrapper and `build.mjs` bundler). Instead of `wasm-pack`, Webpack, or Parcel, packaging is driven directly by `web/lib/build.mjs` using `cargo`, `wasm-bindgen`, `wasm-opt` (`binaryen`), `esbuild`, and `tsc`.

## Build Pipeline (`web/lib/build.mjs`)

Running `just web-lib build` (or `node build.mjs` inside `web/lib/`) executes five deterministic stages:

1. **Exact `wasm-bindgen-cli` version check (`checkWasmBindgenVersion`)**:
   Reads the exact pinned `wasm-bindgen = "=..."` version from the workspace `Cargo.toml` and verifies `wasm-bindgen --version` matches before compiling, preventing subtle JS/Wasm ABI mismatches.
2. **Rust compilation (`cargo build --target wasm32-unknown-unknown`)**:
   Builds `web` with `--profile wasm-dev` (default) or `--profile wasm-release` (`--release`):
   - **WebGPU (default)**: compiles `web` against `wgpu`'s WebGPU backend.
   - **WebGL2 (`--webgl`)**: passes `--features web-webgl` to enable `wgpu` WebGL2 support and downlevel limits.
   - **Multithreaded shared-memory workers (`--multithreaded`)**: passes `-Z build-std=std,panic_abort` and configures `target.wasm32-unknown-unknown.rustflags` with:
     - `-C target-feature=+atomics,+bulk-memory,+mutable-globals`
     - `-C link-args=--shared-memory --import-memory --max-memory=209715200`
     - `-C link-args=--export=__heap_base --export=__wasm_init_tls --export=__tls_size --export=__tls_align --export=__tls_base`
3. **`wasm-bindgen` & automated compatibility checks**:
   Runs `wasm-bindgen --target web --typescript --out-name maplibre --out-dir src/wasm` (plus `--debug` in dev mode). Using `--target web` preserves `wasm_bindgen::module()` so compiled `WebAssembly.Module` handles can be posted directly to Web Workers. Immediately after binding generation, `build.mjs` runs `node --test test/wasm-instance.test.mjs test/webgpu-compat.test.mjs` to validate Wasm instantiation and WebGPU adapter/descriptor compatibility.
4. **Release binary optimization (`wasm-opt`)**:
   When `--release` is passed, optimizes `src/wasm/maplibre_bg.wasm` in place via `wasm-opt -O` (provided by the `binaryen` npm devDependency).
5. **JavaScript bundling (`esbuild`) & TypeScript declarations (`tsc`)**:
   Bundles `src/index.ts` for `platform: "browser"` with `define: { WEBGL, MULTITHREADED }`, `assetNames: "assets/[name]"`, and two plugins:
   - `esbuild-plugin-inline-worker` — bundles and inlines Web Worker entrypoints (both message-passing `FlatBufferTransferable` workers and shared-memory pool workers) as ES2022 CJS worker blobs so consumers do not have to configure separate worker asset routes.
   - `@chialab/esbuild-plugin-meta-url` — resolves `new URL(..., import.meta.url)` references (such as `maplibre_bg.wasm`) and emits the `.wasm` binary to `dist/esbuild-<format>/assets/` with portable runtime URL resolution.

   Finally, `tsc -m es2022 -outDir dist/ts --declaration --emitDeclarationOnly` writes TypeScript `.d.ts` declarations to `web/lib/dist/ts/`.

## Output Bundle Formats

`web/lib/build.mjs` supports three output module formats, selectable individually or together:

| Flag | Output Path | Format Details |
| --- | --- | --- |
| `--esm` *(default)* | `web/lib/dist/esbuild-esm/module.js` | Standard ES module bundle for modern bundlers and native `<script type="module">` imports. Enabled automatically when neither `--cjs` nor `--iife` is specified. |
| `--cjs` | `web/lib/dist/esbuild-cjs/module.js` | CommonJS bundle for legacy bundlers and CommonJS-based test runners. |
| `--iife` | `web/lib/dist/esbuild-iife/module.js` | Immediately-invoked function expression exposing the global `window.maplibre` namespace for `<script>` tag embeds and standalone playgrounds, resolving `assets/maplibre_bg.wasm` relative to `document.currentScript.src`. |

## Backend & Threading Matrix

| Variant | Command | Browser Requirements |
| --- | --- | --- |
| **WebGPU (single-memory workers)** | `just web-lib build --release` | Browser with WebGPU (`navigator.gpu`); Web Workers exchange decoded tiles via zero-copy transferable buffers (`FlatBufferTransferable`). |
| **WebGL2 (single-memory workers)** | `just web-lib build --release --webgl` | Any WebGL2 browser; works without special cross-origin isolation headers. |
| **WebGL2 + Multithreaded Wasm** | `just web-lib build --release --webgl --multithreaded` | Requires nightly Rust (`-Z build-std`) at build time and a [secure context](https://developer.mozilla.org/en-US/docs/Web/API/isSecureContext) with [`crossOriginIsolated`](https://developer.mozilla.org/en-US/docs/Web/API/crossOriginIsolated) (`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`) at runtime for `SharedArrayBuffer`. |

Direct `build.mjs` invocations accept any combination of flags:

```bash
cd web/lib
npm ci
node build.mjs --release --webgl --esm --cjs --iife
```

## Wasm Dependency Hygiene Guard

Because `maplibre` disables `native-io` on `wasm32-unknown-unknown`, no native async runtime, socket, TLS, or SQLite crate is permitted in the browser build. CI enforces this on every pull request via `python3 scripts/check-wasm-deps.py`, which inspects `cargo tree -p web --target wasm32-unknown-unknown -e normal` (both default WebGPU and `--features web-webgl`) and fails if `tokio`, `mio`, `hyper`, `rusqlite`, `openssl`, `openssl-sys`, `rustls`, `socket2`, `curl`, `reqwest-middleware`, or `http-cache-reqwest` appear anywhere in the normal dependency graph (or if `reqwest` appears in `maplibre`'s own `wasm32` graph).
