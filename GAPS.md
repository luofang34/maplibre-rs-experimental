# Gaps

What `maplibre-rs` lacks or only partly has, measured against the MapLibre Style Specification and
against [mln-rs](https://github.com/louwers/mln-rs), the Rust port of MapLibre Native. Delete an
entry when it is closed.

Sizes: **S** days, **M** weeks, **L** a month or more. Paths are relative to `maplibre/src` unless
they start with another directory.

## Open defects

| Defect | Where | Fix |
| --- | --- | --- |
| A memo keyed on `Arc` address drops the replaced `Arc`, so a reused address can give one feature another feature's properties (web wire path) | `style/feature_state/sync.rs` `sync_arc_properties_feature_state`; memo created in `vector/populate_world_system.rs` | Keep the original `Arc` alive in the memo entry or key on content; add a web test with two messages in one batch |
| Property maps of id-bearing features are cloned on the main thread at arrival when the source has no feature-state (unverified) | `style/feature_state/sync.rs` `properties_need_feature_state_sync` | Write the state id in the worker, or skip the sync when the source has no state and no `promoteId` |
| A property entry is retained for every feature of every non-symbol layer; the web path duplicates maps per style layer | `vector/tessellation/feature_paint.rs`, `vector/content.rs`, `vector/feature_paint/wire.rs` | Retain only for layers whose paint is data-driven or reads feature-state; share one decoded map per feature on web |

## Conformance evidence and CI

| Gap | State | Size |
| --- | --- | --- |
| Render suite gated in CI | `render-tests/` vendors the GL JS fixtures; no workflow runs them and no pass rate is recorded | M |
| Ignore list with reasons for failing render cases | None | S |
| Query cases (`expected.json`) gated | Not gated | S |
| Expression cases for `at`, `split`, `join`, `error`, `within`, `distance`, `collator`, `number-format`, `feature-state`, `global-state` | Absent from the vendored suite; `tests/expression_conformance.rs` asserts a floor, not a named expected-failure list | S |
| `headless` integration tests and six book examples compiled by a native CI job | Not run by any workflow | S |
| CI build without `native-io` | None | S |
| CI for `apple/visionos` | None | M |
| Sanitizer job; Vulkan validation and Metal debug layers in test jobs | None | M |
| `scripts/check-module-layout.py`, `scripts/check-immersive-stability.py` in CI | Not wired | S |
| Coverage upload, MSRV job, `cargo-deny` | None | S |
| Benchmark comparison against a base ref | Criterion benches run without comparison | M |

## Text and symbols

| Gap | State | Size |
| --- | --- | --- |
| Complex shaping (Indic, ligatures, kerning from font tables) | Missing; no shaper or font parser. `style/expression/collation.rs` lists `UNSUPPORTED_SCRIPTS` | L |
| Arabic shaping | Partial; presentation-form table in `style/arabic_shaping.rs` | with above |
| Font files (TTF, WOFF, WOFF2), `font-faces` style root, system fallback fonts | Missing | M–L |
| Local glyph rasterisation for CJK ideographs | Missing; fixtures in `render-tests/src/tests/text-local-glyphs` | M |
| `symbol-avoid-edges` | Missing | S |

## Storage and offline

| Gap | State | Size |
| --- | --- | --- |
| Offline regions and download | Missing; host-supplied tiles only (`headless/map/injected_loader.rs`) | L |
| Tile cache in the browser build | Missing; `platform/web/http_client.rs` is plain `fetch` | M |
| MBTiles at runtime | Missing; build-time embedding only (`maplibre-build-tools/src/mbtiles.rs`) | S–M |
| Request and header transform hooks | Partial; only by replacing `ResourceLoader` or `HttpClient` | S |
| Network status and offline mode | Missing | S |
| Source `volatile` | Missing | S |

## Style specification and map API

| Gap | State | Size |
| --- | --- | --- |
| Property transitions (`*-transition`, root `transition`) | Missing in the core; `render-tests/src/transitions.rs` interpolates in the harness | M |
| Whole-style replacement with diffing | Missing; granular mutations only (`style/mutation.rs`) | M |
| Camera bounds: fit to bounds, max bounds, min and max zoom | Missing; only `set_max_pitch` | M |
| Typed event stream (load, idle, error, style and source changes) | Partial; polled signals in `map/signals.rs`, `render/frame_signals.rs` | M |
| Expression value types `padding`, `numberArray`, `colorArray`, `projectionDefinition`, `variableAnchorOffsetCollection` | Unsupported (`tests/expression_conformance.rs` `UNSUPPORTED_TYPES`) | M |
| `accumulated` outside cluster properties | Partial (`geojson/cluster.rs`) | S |
| Locale data for `collator` and `number-format` | Partial; own tables, no ICU | M |
| Style-level custom layer | Partial; plugins and host GPU only | M |
| `styleimagemissing` event | Partial; pull-based `StyleImageProvider` | S |
| Gesture recogniser independent of winit | Partial; handlers live in `maplibre-winit/src/input` | M |
| `resampling` on hillshade and color-relief, `line-round-limit`, `fill-extrusion-rounded-corner-distance`, GeoJSON `lineMetrics` and `tolerance` | Missing | S each |
| `video` and `canvas` sources | Missing | M |

## Platforms and embedding

| Gap | State | Size |
| --- | --- | --- |
| General C ABI | Partial; visionOS only (`apple/visionos/maplibre_visionos.h`) | L |
| Thread-safe runtime and map handles | Partial; single-thread `MapHandle` (`map/handle.rs`) | M–L |
| Android and iOS as embeddable SDKs | Partial; `android/src/lib.rs` and `apple/src/lib.rs` start the demo map | M |
| Guard on the wasm build's dependency set | Missing | S |
| Book | Architecture and platform pages predate the TCS, schedule, plugin, headless and XR design; `docs/src/user-guide/index.md` sets a field that does not exist (`auto_tilt_override`); no visionOS page; `docs/src/SUMMARY-book.md` lists a missing `developer-log.md` | M |

## Architecture and engineering

| Gap | State | Size |
| --- | --- | --- |
| Crate layering | One crate holds the engine, with 34 `pub mod` | L |
| Workspace lint policy | No `[workspace.lints]`; `unsafe_code`, `unwrap_used`, `expect_used` are not denied in the core crate | M |
| Panic sites | Baselined per crate in `scripts/panic-baseline.json`, mostly `.expect`; no per-file ratchet | M |
| Executor at the edge | Tokio reaches past `platform/noweb` into `io/cpu_work.rs` and `headless/map/` | L |
| Global state | `SHARED_CACHE_FRONTENDS`, `IN_PROCESS_HANDLERS` (`platform/noweb/http_client.rs`), `LIVE_BYTES` (`tcs/system/heap.rs`) | S–M |
| `unsafe` in the ECS | `tcs/tiles.rs`, `tcs/resources.rs` | M |
| `experimental` feature on by default | Gates celestial, starfield and overlay code | S |
| Style types generated from the spec JSON | Hand-written (`style/layer/paint.rs`, `style/property/`) | M |
| Releases | No tags, no publish pipeline | M |
| Leftovers | Six `mod.rs` files; `raster` and `geojson` features gate nothing; `maplibre/style-spec-v8.json` is unreferenced; edition 2021 | S |
