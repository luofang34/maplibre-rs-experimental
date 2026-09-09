# Symbol rendering and style contract

The default terrain style distinguishes countries, regions, major cities, cities, towns, villages and neighbourhoods with entry zoom, continuous size, spacing, weight and collision padding. Major cities and countries use Noto Sans Bold; secondary places use Regular, and water uses Italic. The glyph service must supply the exact font stack. Halos remain smaller than one tenth of the default type size. Land and water colors are independent of source tile zoom.

The Rust renderer evaluates the shared `symbol-height-offset` during placement at the display zoom, including feature expressions. `symbol-height-anchor: ground` adds sampled terrain; `absolute` uses the supplied altitude. Text and icons share that height for GPU placement and CPU collision/query geometry. Component height aliases remain supported for existing clients. Feature-state driven height and live style mutation require additional lifecycle work; these changes do not claim that support.

SDF edge coverage uses a Euclidean pixel gradient so stroke softness does not grow solely because a label rotates diagonally. Text halo width is capped at one quarter of rendered font size; zero width disables halo coverage so its color cannot thicken antialiased edges. Tracking adds space between glyphs, without an extra trailing gap that shifts centered text. Single-sample pixel tests exercise rotated strokes, zoom-dependent shared heights on parent geometry, terrain occlusion, selectable text, and stationary frames.

## Compatibility still to implement

- Variable-anchor candidate placement, radial offsets and variable-anchor-offset precedence.
- Fully feature-driven color, opacity, font and size uniforms; formatted sections, inline images and complete multilingual shaping parity.
- Cross-tile identity and animated collision transitions, with one placement decision for both eyes. Current cached placement and ready-parent coverage keep stationary frames stable; they are not the GL JS placement/fade algorithm.
- Per-glyph curved line placement and corresponding collision boxes; current line labels use a tangent at the anchor.
- Query parity for GPU-occluded symbols. The current query uses CPU placed bounds; shader depth rejection alone does not remove an occluded feature from those results.
- Image mipmaps and small-icon filtering, verified separately from SDF glyph filtering.

Each property needs an expression/layout test and a rendered fixture before it can be listed as supported. Query changes also need visible/occluded, icon/text and duplicate-feature tests. Keep generic rendering work in the private maplibre-rs fork; platform input and aviation display policy remain host responsibilities.

References: [MapLibre symbol properties](https://maplibre.org/maplibre-style-spec/layers/#symbol), [GL JS text shaping](https://github.com/maplibre/maplibre-gl-js/blob/main/src/symbol/shaping.ts), [GL JS glyph quads](https://github.com/maplibre/maplibre-gl-js/blob/main/src/symbol/quads.ts).
