# Gaps

What `maplibre-rs` lacks or only partly has, measured against the MapLibre Style Specification and
against [mln-rs](https://github.com/louwers/mln-rs), the Rust port of MapLibre Native. Delete an
entry when it is closed.

Sizes: **S** days, **M** weeks, **L** a month or more. Paths are relative to `maplibre/src` unless
they start with another directory.

## Text and symbols

| Gap | State | Size |
| --- | --- | --- |
| Complex shaping (Indic, ligatures, kerning from font tables) | Missing; no shaper or font parser. `style/expression/collation.rs` lists `UNSUPPORTED_SCRIPTS` | L |
| Arabic shaping | Partial; presentation-form table in `style/arabic_shaping.rs` | with above |
| Font files (TTF, WOFF, WOFF2), `font-faces` style root, system fallback fonts | Missing | M–L |
| Local glyph rasterisation for CJK ideographs | Missing; fixtures in `render-tests/src/tests/text-local-glyphs` | M |
| `symbol-avoid-edges` | Missing | S |

## Style specification and map API

| Gap | State | Size |
| --- | --- | --- |
| Property transitions (`*-transition`, root `transition`) | Missing in the core; `render-tests/src/transitions.rs` interpolates in the harness | M |
| Whole-style replacement with diffing | Missing; granular mutations only (`style/mutation.rs`) | M |
| `resampling` on hillshade and color-relief, `line-round-limit`, `fill-extrusion-rounded-corner-distance`, GeoJSON `lineMetrics` and `tolerance` | Missing | S each |
| `video` and `canvas` sources | Missing | M |

## Architecture and engineering

| Gap | State | Size |
| --- | --- | --- |
| Crate layering | One crate holds the engine, with 34 `pub mod` | L |
