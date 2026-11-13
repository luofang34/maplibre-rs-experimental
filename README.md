<h1 style="text-align:center">
  <img width="350px" alt="maplibre-rs" src="./docs/logo/maplibre-rs-with-text.svg">
</h1>

<div style="text-align:center">
  <strong>Native Maps for Web, Mobile and Linux</strong>
</div>
<div style="text-align:center">
  A map rendering library written in Rust.
</div>

<div style="text-align:center">
  <img src="https://img.shields.io/badge/stability-experimental-orange.svg?style=flat-square"
      alt="Stability" />
  <a href="https://github.com/maplibre/maplibre-rs/actions/workflows/on_main_push.yml">
    <img src="https://github.com/maplibre/maplibre-rs/actions/workflows/on_main_push.yml/badge.svg"
        alt="Build status" />
  </a>
  <a href="https://matrix.to/#/#maplibre:matrix.org">
    <img src="https://img.shields.io/static/v1?label=Space&message=%23maplibre&color=blue&logo=matrix"
        alt="Build status" />
  </a>
</div>

<div style="text-align:center">
  <h3>
    <a href="https://webgl.demo.maplibre-rs.maplibre.org">
      WebGL Demo
    </a> |
    <a href="https://maplibre.org/maplibre-rs/docs/book/">
      Book
    </a> |
    <a href="https://maplibre.org/maplibre-rs/docs/api/maplibre/">
      API
    </a> |
    <a href="https://slack.openstreetmap.us/">
      Chat (#maplibre-rs)
    </a>
  </h3>
</div>

## Project

`maplibre-rs` is a pure-Rust, data-oriented 2D/3D vector and raster map rendering engine built on [`wgpu`](https://wgpu.rs/) and WGSL, targeting MapLibre Style Specification conformance across desktop, mobile, XR/spatial, headless server, and WebAssembly environments.

## Description

`maplibre-rs` combines a tile-component system (`TCS`), staged `Schedule` execution, slab-pooled GPU geometry/metadata buffers, and a modular `RenderGraph` on top of [WebGPU](https://www.w3.org/TR/webgpu/) (`wgpu`). A single WGSL shader and Rust rendering pipeline runs natively over Metal, Vulkan, DirectX 12, OpenGL ES, and browser WebGPU/WebGL2.

Unlike legacy C++ map engines bound to fixed single-camera class hierarchies, `maplibre-rs` treats 3D globe projection, 3D terrain elevation draping, stereo/multi-view XR rendering, upload/memory budgets, and headless offscreen capture as first-class render-graph capabilities.

## Demos

- [WebGL](https://webgl.demo.maplibre-rs.maplibre.org)
- [WebGL with multithreading](https://webgl-multithreaded.demo.maplibre-rs.maplibre.org) - Does not work on Safari right now
- [WebGPU](https://webgpu.demo.maplibre-rs.maplibre.org) - Supported in modern Chromium, Firefox, and Safari browsers

## Current Features

* **Cross-Platform & Embeddable `wgpu` Engine**: Runs on macOS, Linux, iOS, visionOS (`apple/`), Android (`android/`), WebAssembly (`web/`), and headless GPU/CPU offscreen targets (`headless` feature).
* **Runtime-Agnostic Core (`native-io` Optional Feature)**: Disable default features (`--no-default-features`) to embed `maplibre` in custom event loops, game engines, or Wasm hosts without pulling in `tokio` or `reqwest`.
* **Projections & 3D Terrain**: Flat Mercator and 3D Globe projections, raster-DEM 3D terrain mesh draping (`TerrainPlugin`), hillshade, color-relief, atmosphere, and celestial sky rendering.
* **Style Layer Coverage**: `background`, `fill` (fast `earcut` polygon triangulation with `lyon` fallback and `fill-outline-color` / patterns), `line` (dashes, gradients, patterns, variable widths), `circle`, `heatmap`, `fill-extrusion` (3D buildings), `raster`, `hillshade`, `color-relief`, and `symbol` (SDF & bitmap glyphs, icons, variable text anchors, curved line labels, and screen-space collision detection).
* **Data Sources & Tile Formats**: Mapbox Vector Tiles (MVT with pure-Rust `zlib-rs` decompression), MapLibre Tiles (MLT), GeoJSON (inline & tiled, `promoteId` & `generateId`), PMTiles archives, raster tiles, and Terrain-RGB / Terrarium DEM tiles.
* **Style Specification Expressions & Dynamic State**: Full expression parser, type checker, constant folder, and evaluator (`let`/`var`, `match`, `case`, `coalesce`, `interpolate`/`interpolate-hcl`/`interpolate-lab`, `step`, `format`, `collator`, `number-format`, `within`, `distance`, `at`, `split`, `join`, `feature-state`, `global-state`).
* **In-Place GPU Style & `feature-state` Updates**: Paint mutations, layer reordering, zoom-range/visibility edits, `set_global_state`, and `set_feature_state` / `remove_feature_state` refresh per-feature GPU metadata buffers in place without re-fetching or re-tessellating tiles.
* **Spatial Feature Queries & Multi-View Rendering**: `query_rendered_features` (returning source properties and live `feature-state` maps) over R-tree indexed tile geometry, plus stereo/multi-view camera rendering for spatial computing.

## Roadmap / Remaining Gaps

* Complex right-to-left / Indic text shaping (`harfbuzz` / `harfrust`) and runtime vertical CJK punctuation layout
* Non-scalar expression interpolation types (`padding`, `variableAnchorOffsetCollection`, `projectionDefinition`)
* Further splitting the core `maplibre` crate into smaller workspace sub-crates for faster incremental downstream builds

## Building & Running

Clone the project

```bash
git clone https://github.com/maplibre/maplibre-rs.git
```

Build and run it on a desktop

```bash
cargo run -p maplibre-demo
```

Render the world on a globe instead of the flat default style

```bash
cargo run -p maplibre-demo -- headed --globe
cargo run -p maplibre-demo -- headed --terrain
cargo run -p maplibre-demo -- headed --style path/to/style.json
```

More information about running the demos on different platforms can be
found [here](https://maplibre.org/maplibre-rs/docs/book/development-guide/how-to-run.html).

## Rust Setup

Install [rustup](https://rustup.rs/) because this is the recommended way of setting up Rust toolchains.

The toolchain will be automatically downloaded when building this project.
See [./rust-toolchain.toml](./rust-toolchain.toml) for more details about the toolchain.

## API Documentation

This generates the documentation for this crate and opens the browser. This also includes the documentation of every
dependency.

```bash
cargo doc --open
```

You can also view the up-to-date documentation [here](https://maplibre.org/maplibre-rs/docs/api/maplibre/).

## Book

The maplibre-rs [book](https://maplibre.org/maplibre-rs/docs/book/) features a high-level overview over the project from a user and development perspective.

## RFCs

We established an RFC process which must be used to describe major changes to maplibre-rs.
Current RFCs can be browsed in the [book](https://maplibre.org/maplibre-rs/docs/book/rfc/0001-rfc-process.html).


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
