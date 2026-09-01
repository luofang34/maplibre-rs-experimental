# Globe projection parity matrix

This matrix audits the MapLibre GL JS `projection/globe` render corpus against maplibre-rs. It
separates projection work from layer features that the Rust renderer does not yet implement on a
flat map. The audit contains 75 non-terrain fixtures and 45 terrain fixtures. Terrain fixtures are
tracked by count here and belong to the terrain RFC.

Status meanings:

- **Implemented**: the projection path and automated CPU/WGSL evidence exist.
- **Partial**: a working projection path exists, but a GL JS behavior or golden render remains.
- **Baseline blocker**: the layer/source/API is not sufficiently implemented in the Mercator path.
- **Deferred**: intentionally belongs to the terrain feature.

## Non-terrain corpus

| GL JS fixture family | Cases | Status | maplibre-rs evidence or blocker |
| --- | ---: | --- | --- |
| `background`, `background-opacity` | 2 | Implemented | Curved z0 mesh without borders or stencil; style color/opacity path |
| `background-pattern` | 1 | Baseline blocker | Background patterns are not implemented independently of projection |
| `fill-planet-*`, `fill-seams/*`, `fill-translate` | 7 | Partial | Polygon subdivision, pole rows, antimeridian clipping, fill WGSL; golden seams and translate remain |
| `line-*` excluding patterns/dashes/gradients | 2 | Partial | Line subdivision and projected width exist; spiral/translate golden renders remain |
| line pattern/dash/gradient families | 8 | Baseline blocker | Complete pattern, dash, and gradient paint paths are absent |
| `raster-*` | 3 | Partial | Curved raster mesh, border policy, poles, and projection pipeline exist; golden renders require GPU |
| `image*` | 2 | Baseline blocker | Image-source rendering is absent |
| symbol/text/collision families | 22 | Partial | Projected anchors, collision opacity, horizon rejection, and antimeridian tests exist; line placement, variable anchors, and translate parity remain |
| `text-always-overlap-occluded/*` | 3 | Implemented structurally | CPU horizon culling runs even when overlap is allowed; golden renders require GPU |
| `circle-*` | 5 | Baseline blocker | Circle rendering is not wired into the active Rust render graph |
| `fill-extrusion*` | 2 | Baseline blocker | Fill-extrusion rendering is absent |
| `heatmap` | 1 | Baseline blocker | Heatmap rendering is absent |
| `hillshade` | 1 | Baseline blocker | Hillshade rendering is absent |
| `custom/*` | 3 | Baseline blocker | Custom-layer projection API and renderer are absent |
| `atmosphere/*`, `sky` | 10 | Partial | Atmosphere blend expressions and pass ordering exist; physical scattering and light-position inputs remain |
| `antimeridian-lod` | 1 | Implemented structurally | Wrapped covering, variable LOD, pitch, and rotation reference fixtures |
| `antimeridian-overdraw/*` | 11 | Partial | Geometry clipping and canonical wraps exist; affected paint families and golden renders remain |
| `zoom-transition` | 1 | Implemented structurally | Endpoint and intermediate projection-expression, covering, and shader tests |

The family counts overlap where a fixture tests more than one subsystem, so their sum is not a
fixture count. The authoritative audited total is 75.

## Terrain corpus

All 45 `projection/globe/terrain` fixtures are **Deferred**. The globe implementation already uses
radial elevation math and elevation-aware covering volumes, but it does not claim DEM upload,
terrain mesh displacement, depth integration, screen picking, atmosphere composition, or golden
render parity.

## Automated checks available without a GPU

The local gate covers:

- style parsing and projection-expression evaluation;
- f64 globe math and GL JS reference values;
- camera matrices, projection/unprojection, horizon, and occlusion;
- tile mesh topology, borders, poles, and index-width safety;
- vector subdivision and antimeridian clipping;
- tile covering, wrap selection, frustum culling, and variable LOD;
- projection uniform layout and WGSL validation through Naga;
- render-phase ordering and pipeline descriptor construction;
- symbol horizon/canonical-tile behavior;
- atmosphere style evaluation;
- versor interaction stability at poles and the antimeridian.

The local Metal/wgpu backend runs the headless harness and the globe pipeline successfully.
Golden-image acceptance still requires importing the corresponding GL JS styles, assets, camera
operations, and expected images; a successful pipeline probe is not a golden parity result.
