<h1 style="text-align:center">
  <img width="350px" alt="maplibre-rs" src="./docs/logo/maplibre-rs-with-text.svg">
</h1>

<div style="text-align:center">
  <strong>Maps for desktop, web, mobile, spatial and headless hosts</strong>
</div>
<div style="text-align:center">
  A map rendering library written in Rust.
</div>

<div style="text-align:center">
  <img src="https://img.shields.io/badge/stability-experimental-orange.svg?style=flat-square"
      alt="Stability" />
  <a href="https://github.com/luofang34/maplibre-rs-experimental/actions/workflows/on_main_push.yml">
    <img src="https://github.com/luofang34/maplibre-rs-experimental/actions/workflows/on_main_push.yml/badge.svg"
        alt="Build status" />
  </a>
</div>

<div style="text-align:center">
  <h3>
    <a href="https://luofang34.github.io/maplibre-rs-experimental/docs/book/">
      Book
    </a> |
    <a href="https://luofang34.github.io/maplibre-rs-experimental/docs/api/maplibre/">
      API
    </a>
  </h3>
</div>

## Project

`maplibre-rs` is a pure-Rust, data-oriented 2D/3D vector and raster map rendering engine built on [`wgpu`](https://wgpu.rs/) and WGSL. It targets MapLibre Style Specification conformance, takes MapLibre GL JS as its behavioural reference, and runs on desktop, mobile, XR/spatial, headless and WebAssembly hosts.

This repository is an experimental fork of [maplibre-rs](https://github.com/maplibre/maplibre-rs). No crate is published and the API is unstable.

## Description

`maplibre-rs` combines a tile-component system (`TCS`), staged `Schedule` execution, plugins, slab-pooled GPU geometry and metadata buffers, and a modular `RenderGraph` on top of [WebGPU](https://www.w3.org/TR/webgpu/) (`wgpu`). One WGSL shader set and one Rust rendering pipeline run over Metal, Vulkan, DirectX 12, OpenGL ES, and browser WebGPU/WebGL2.

Globe projection, terrain draping, stereo and multi-view rendering, upload and memory budgets, and offscreen capture are render-graph capabilities of the same engine, not separate code paths.

## Features

* **Platforms**: macOS, Linux and Windows through `maplibre-winit` and `maplibre-demo`; WebAssembly (`web/`) on WebGPU or WebGL2, single- or multi-threaded; Android (`android/`) and iOS/macOS (`apple/`) launchers; visionOS (`apple/visionos`) as a C ABI static library with a Swift host app; headless offscreen rendering (`headless` feature).
* **Embedding**: `HeadlessMap` renders on its own device or on a host-supplied `wgpu` device, into host textures, for one view or several view slots sharing one tile world (`run_multi_view_frame`, `run_xr_frame`). A host reports free memory with `set_available_memory`, and the engine lowers drape, in-flight tile and upload budgets under pressure.
* **Optional native I/O**: the default `native-io` feature supplies the Tokio scheduler and Reqwest HTTP client. Without it a native host supplies its own scheduler and tile loader.
* **Projections and terrain**: Mercator, globe and vertical-perspective projections, including projection expressions; raster-DEM terrain with draping and exaggeration; hillshade, color-relief, sky and atmosphere; stars and celestial bodies (`experimental` feature).
* **Layers**: `background`, `fill` (`earcut` triangulation with a `lyon` fallback, outlines, patterns), `line` (dashes, gradients, patterns, data-driven widths), `circle`, `heatmap`, `fill-extrusion`, `raster`, `hillshade`, `color-relief` and `symbol` (SDF and bitmap glyphs, icons, variable anchors, line labels, collision detection).
* **Sources and formats**: vector tiles as MVT or MLT (`mlt` feature), GeoJSON (inline or by URL, `promoteId`, `generateId`, clustering), raster, raster-dem (Mapbox Terrain-RGB and Terrarium), image sources, and PMTiles archives through `pmtiles://`.
* **Expressions**: parser, type checker, constant folder and evaluator for the style-spec operators, including `let`/`var`, `match`, `case`, `coalesce`, `interpolate`, `interpolate-hcl`, `interpolate-lab`, `step`, `format`, `collator`, `number-format`, `within`, `distance`, `at`, `split`, `join`, `error`, `feature-state` and `global-state`.
* **Runtime styling**: add, remove and move layers; `set_paint_property`, `set_layout_property`, `set_filter`, `set_layer_zoom_range`; add and remove sources; `set_geojson_data`; `set_global_state`; `set_feature_state` and `remove_feature_state`. Paint, ordering, visibility and feature-state edits that leave geometry unchanged update GPU buffers in place without re-fetching tiles.
* **Queries and camera**: `query_rendered_features`, `query_rendered_symbols` and `query_source_features`; `jump_to` and `fly_to`; constrained and free-globe navigation modes.
* **Text**: bidirectional text (`unicode-bidi`), Arabic presentation-form shaping and `text-writing-mode: vertical`.

## Building & Running

Clone the project

```bash
git clone https://github.com/luofang34/maplibre-rs-experimental.git
```

Open an interactive map window

```bash
cargo run -p maplibre-demo -- headed
```

Choose a bundled style or load your own

```bash
cargo run -p maplibre-demo -- headed --globe
cargo run -p maplibre-demo -- headed --terrain
cargo run -p maplibre-demo -- headed --style path/to/style.json
```

`headed` also accepts `--projection globe|mercator|vertical-perspective`, `--source-url NAME=URL_OR_PATH`, `--max-pitch`, `--frames` and `--debug-tiles`; `--help` lists every option.

Render offscreen with the `headless` feature

```bash
cargo run -p maplibre-demo --features headless -- snapshot --help
cargo run -p maplibre-demo --features headless -- headless
```

The [book](https://luofang34.github.io/maplibre-rs-experimental/docs/book/development-guide/how-to-run.html) describes running the demos on the other platforms.

## Cargo Features

Features of the `maplibre` crate:

| Feature | Default | Purpose |
| --- | --- | --- |
| `native-io` | yes | Tokio scheduler and Reqwest HTTP client on native targets |
| `mlt` | yes | MapLibre Tiles decoding |
| `experimental` | yes | Celestial bodies, starfield and overlay plugins |
| `headless` | no | Offscreen rendering, readback and `HeadlessMap` |
| `web-webgl` | no | WebGL2 backend for the browser |
| `thread-safe-futures` | no | `Send` futures for multi-threaded hosts |
| `trace` | no | Tracy profiling |
| `embed-static-tiles` | no | Tiles embedded at build time |

## Rust Setup

Install [rustup](https://rustup.rs/) because this is the recommended way of setting up Rust toolchains.

The workspace builds on the stable toolchain named in [./rust-toolchain.toml](./rust-toolchain.toml), which rustup downloads automatically. The web and Android recipes in the [justfile](./justfile) and the visionOS crate use nightly.

## API Documentation

```bash
cargo doc -p maplibre --no-deps --open
```

The documentation of the main branch is published [here](https://luofang34.github.io/maplibre-rs-experimental/docs/api/maplibre/).

## Book

The [book](https://luofang34.github.io/maplibre-rs-experimental/docs/book/) gives an overview of the project for users and developers, with runnable examples.

## RFCs

Major changes are described through the RFC process in [rfc/0001-rfc-process.md](rfc/0001-rfc-process.md).


## License

This repository is a fork of [maplibre-rs](https://github.com/maplibre/maplibre-rs), taken at
upstream commit `96b50a09d4925f46bd68e184e754d22ac3e356a2`.

- Code as it stands at that upstream commit is licensed under either [MIT](LICENSE-MIT) or
  [Apache-2.0](LICENSE-APACHE), at your option.
- Changes and additions made in this fork since that commit are Copyright (C) 2026 Fang Luo and
  Sokoly Systems, and licensed under the [GNU Affero General Public License, version 3 or any
  later version](LICENSE-AGPL). The fork as a whole is therefore distributed under
  AGPL-3.0-or-later; the upstream code it contains remains available under its own licenses.
- Third-party material bundled in the fork keeps its own license:
  - render fixtures and assets under `render-tests/` taken from maplibre-gl-js, under the terms
    maplibre-gl-js distributes them on (see
    [render-tests/LICENSE-maplibre-gl-js.txt](render-tests/LICENSE-maplibre-gl-js.txt)); the map
    data, imagery, sprites and fonts within them remain under their owners' terms;
  - expression conformance cases under `maplibre/tests/expressions` taken from
    maplibre-style-spec: BSD-3-Clause, see
    [LICENSE-maplibre-style-spec.txt](maplibre/tests/expressions/LICENSE-maplibre-style-spec.txt);
  - the Indicate snapshot under `apple/visionos/MapLibreVision/IndicateOverlay/vendor/indicate`,
    from the repository its `SOURCE.json` records, is not covered by this repository's licenses;
    no license has been granted for it;
  - the replay data listed in
    [ATTRIBUTION.txt](apple/visionos/MapLibreVision/MapLibreVision/Resources/ATTRIBUTION.txt),
    under the terms stated there.

## Acknowledgements

The renderer of maplibre-rs is heavily based on the renderer of [bevy](https://bevyengine.org/). Bevy's renderer was 
forked into this project in order to have a solid and generic base.

## Articles and talks about maplibre-rs
- Aug. 2022 - [maplibre-rs: toward portable map renderers](https://doi.org/10.5194/isprs-archives-XLVIII-4-W1-2022-35-2022)
