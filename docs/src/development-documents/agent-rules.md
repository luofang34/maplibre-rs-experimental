# Agent Rules, Architecture Gates & Verification Workflow

`maplibre-rs-experimental` enforces unified Rust, WebAssembly, and TypeScript/JavaScript static and verification gates through [`evidence.toml`](../../../evidence.toml) (`cargo-evidence` v0.2, `Tier 2: verify`) and `evidence-mcp`.

## 1. Progressive Context Discovery (Read On Demand)

Before reading broad directory trees or planning an edit, query the active feature slice, ownership boundaries, ceilings, and verification targets for the file or symbol you are modifying:

```bash
cargo evidence context <path> --json
```

When running in an MCP-enabled client (configured in `.claude/settings.json`), call the `evidence_context` tool with the target file path instead. The returned context identifies:

- The `[feature_map.<slice>]` owning the path (`coords_projection`, `globe_terrain`, `vector_tessellation`, `sdf_symbols`, `apc_io`, `web_platform`, `render_pipeline`).
- Applicable crate layers and WASM dependency closures (`[[architecture.dep_closures]]`).
- Active `[floors.per_file_ceilings]`, `[floors.per_crate_ceilings]`, and `[structure.file_size_allowlist]` bounds.
- Scoped `verify_targets` so you run only the harnesses and crate tests relevant to your slice during iteration.

Load deeper sections of `evidence.toml` or subsystem design docs only when a context query or gate diagnostic points to them.

## 2. Crate Layering & WASM Dependency Closures

Every workspace member belongs to one strict dependency layer (`foundation -> core -> platforms -> apps_and_tests`):

1. **foundation**: `maplibre-build-tools`, `maplibre-coords`, `maplibre-tcs` (zero cycles; no upward dependency on `maplibre`).
2. **core**: `maplibre` (`allow_same_layer = false`).
3. **platforms**: `web-tile-worker`, `web`, `maplibre-winit`, `maplibre-ffi`, `apple`, `maplibre-android`.
4. **apps_and_tests**: `maplibre-demo`, `benchmarks`, `render-tests`.

Five declarative `[[architecture.dep_closures]]` rules inspect `wasm32-unknown-unknown` dependency trees (`web`, `web-webgl`, `web-tile-worker`, `maplibre`, `maplibre-webgl`) and forbid native runtime, socket, TLS, HTTP-middleware, and SQLite crates (`tokio`, `mio`, `hyper`, `rusqlite`, `libsqlite3-sys`, `openssl*`, `rustls`, `socket2`, `curl*`, `reqwest-middleware`, `http-cache-reqwest`), plus `reqwest` in `maplibre`/`web-tile-worker`. Absence of `winit` from single-package `web-tile-worker` (`default-features = false` on `web`) is enforced alongside Clippy via `cargo tree -p web-tile-worker` inside the `wasm32-clippy` verify harness. Place native-only dependencies under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` or behind `native-io` / non-wasm feature flags.

## 3. Lints, Suppression Discipline & Panic Debt Ratchet

- All crates inherit `[workspace.lints]` in `Cargo.toml`: `unsafe_code = "deny"`, `unwrap_used = "deny"`, `expect_used = "deny"`, `panic = "deny"`, `todo = "deny"`, `unimplemented = "deny"`, `indexing_slicing = "deny"`, `await_holding_lock = "deny"`, `let_underscore_must_use = "deny"`, and `let_underscore_future = "deny"`.
- Bare `#[allow(...)]` in non-test code is rejected (`forbid_bare_allow = true`). When a narrow, justified suppression is unavoidable outside tests (for example proc-macro/codegen boundaries or `UploadQueue` internals), use `#[expect(..., reason = "...")]`.
- **Never** silence `clippy::too_many_arguments` with `#[allow]` or `#[expect]` — group parameters into a typed request/spec struct.
- `[floors]` enforces `new_library_panics = 0`, `new_unwraps = 0`, and `new_dead_code_allows = 0` in diffs, and pins `[floors.per_crate_ceilings]` + `[floors.per_file_ceilings]`. External test submodules included from another file must live under `tests/` or use `*_tests.rs` / `tests.rs` / `tests_*.rs` naming so lexical scanners classify them as test code; when burning down `.expect(` debt in `maplibre`, lower the corresponding ceiling in lockstep.
- `[[floors.json_ceilings]]` pins `render-tests/ignores.json` (`/ignores <= 70`). Fix failing fixtures rather than adding ignores.

## 4. Structure & Comment Hygiene

- Maximum `500` lines per non-test `.rs` file (`lib.rs <= 100` with mandatory `//!` crate doc comment) and `80` lines per function body (`[structure]`). Files in `[structure.file_size_allowlist]` may only shrink; `cargo evidence gate` fails with `GATE_ALLOWLIST_ENTRY_STALE` when an entry stops exceeding `500` or its recorded limit.
- Legacy `mod.rs` and catch-all filenames (`utils.rs`, `helpers.rs`, `common.rs`) are forbidden in production source trees, alongside stray artifacts (`frame_*.png`, `* 2.*`, `*.bak`, `*.orig`, `*~`).
- Production comments must explain invariants and non-obvious design constraints only: `[comments]` bans rot/archaeology markers (`previously`, `historically`, `formerly`, `#123` refs), excuse markers (`TODO`, `FIXME`, `HACK`, `WORKAROUND`), sync/apology hedges, inline comment blocks `> 6` consecutive lines, and `> 15%` inline comment density on files `>= 40` lines.

## 5. Web Boundaries & Deterministic Async Lifecycle

- `[[web_boundaries]]` isolates browser workers from main-thread DOM modules (`browser.ts`, `canvas.ts`, `map.ts`, `offscreen-client.ts`, `offscreen-coalesce.mjs`, `index.ts`), keeps tile pool workers (`pool.worker.ts`, `multithreaded-pool.worker.ts`) free of WebGPU/render-worker code, and prevents DOM helpers from importing internal worker bundles directly.
- `[[banned_patterns]]` bans `setTimeout(...)` across `web/**` (except the CLI build watcher in `web/lib/build.mjs`) and bans ad-hoc `requestAnimationFrame(...)` outside `web/lib/src/offscreen-coalesce.mjs` and `web/xr/src/main.ts`. Use explicit Worker completion/destroyed events or `AbortSignal.timeout` instead of arbitrary sleeps or polling timers.

## 6. Verification Harnesses, Disk Budget & Git Hooks

- **Iterate locally with scoped tests**: `cargo test -p <crate>` (for example `cargo test -p maplibre`, `cargo test -p maplibre-coords`, `cargo test -p maplibre-tcs`).
- **Disk & target-directory discipline**: Keep only default `target/` (native builds/tests/benches/render-tests) and `target/verify-wasm` (`wasm32-unknown-unknown` Clippy checks). Never spawn concurrent Cargo builds sharing a target directory or create ad-hoc `CARGO_TARGET_DIR` copies.
- **Static & full verification commands**:
  - `RUSTUP_TOOLCHAIN=nightly cargo fmt -- --check` — formatting parity with CI.
  - `cargo evidence gate --format=jsonl` — Tier 1 static/architecture/floor/web/comment gate (also wired into `.githooks/pre-commit` via `git config core.hooksPath .githooks` and `.claude/settings.json` `PostToolUse`).
  - `cargo evidence check --tier=verify --format=jsonl` — Tier 2 full verification running `wasm32-clippy`, `web-lib` (`tsc` + unit tests), `native-nextest`, `proptest` (`min_proptests >= 16`), and representative `render-tests` frames, indexing screenshots, diffs, and lifecycle telemetry into `artifacts/manifest.json`.
  - `./scripts/verify-phase.sh` — parallel local PR gate delegating static checks to `cargo evidence gate` alongside `check-release-version.py`, `check-immersive-stability.py`, Web/WASM Clippy, and native tests.
