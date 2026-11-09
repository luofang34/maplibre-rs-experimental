# Examples

Each example reproduces the map of a [MapLibre GL JS example](https://maplibre.org/maplibre-gl-js/docs/examples/) with maplibre-rs compiled to WebAssembly.

A map starts from a [style document](https://maplibre.org/maplibre-style-spec/). Where GL JS calls `addSource` and `addLayer` once the style has loaded, an example adds the same source and layer to the document before the map starts. Each example module exports a function returning the document, which the embedded page passes to `startMapLibre`. `startMapLibre` resolves to the running map, whose methods carry GL JS's names, such as `flyTo`, `setPaintProperty` and `queryRenderedFeatures`, and return promises that settle once the map has applied them; an interactive example also exports a `run` function that the page calls with the map:

```js
import { startMapLibre } from 'maplibre-rs'
import style, { run } from './fly-to-a-location.js'

const map = await startMapLibre(undefined, undefined, JSON.stringify(await style()))
run(map)
```

Natively, the demo opens a document saved as JSON:

```bash
cargo run -p maplibre-demo -- headed --style style.json
```

The map reports `click` and `mousemove` through `map.on`, with the point in CSS pixels that `queryRenderedFeatures` takes. Popups and markers of the GL JS examples are left out.

The Rust tab of an interactive example drives the map from Rust in the browser, through the handle that `web::run_map` resolves to.

## Getting Started

- [Display a Map](./display-a-map.md)
- [Display a Satellite Map](./display-a-satellite-map.md)

## Camera

- [Fly to a Location](./fly-to-a-location.md)

## User Interaction

- [Get Features Under the Mouse Pointer](./get-features-under-the-mouse-pointer.md)
- [Change a Layer's Color](./change-a-layers-color.md)
- [Show and Hide Layers](./show-and-hide-layers.md)
- [Filter Symbols by Toggling a List](./filter-symbols-by-toggling-a-list.md)

## Globe

- [Display a Globe with a Vector Map](./display-a-globe-with-a-vector-map.md)
- [Display a Globe with an Atmosphere](./display-a-globe-with-an-atmosphere.md)

## Terrain & Hillshade

- [Add 3D Terrain](./add-3d-terrain.md)
- [Add a Hillshade Layer](./add-a-hillshade-layer.md)
- [Add a Color Relief Layer](./add-a-color-relief-layer.md)

## 3D Models & Buildings

- [Display Buildings in 3D](./display-buildings-in-3d.md)

## Vector & GeoJSON

- [Add a GeoJSON Line](./add-a-geojson-line.md)
- [Add a GeoJSON Polygon](./add-a-geojson-polygon.md)
- [Add Live Realtime Data](./add-live-realtime-data.md)

## Raster & Imagery

- [Add a Raster Tile Source](./add-a-raster-tile-source.md)
- [Add an Image Source](./add-an-image-source.md)

## Expressions

- [Style Lines with a Data-Driven Property](./style-lines-with-a-data-driven-property.md)
- [Create a Gradient Line](./create-a-gradient-line.md)

## Heatmaps & Clusters

- [Create a Heatmap Layer](./create-a-heatmap-layer.md)
- [Create and Style Clusters](./create-and-style-clusters.md)
