# Examples

Each example reproduces the map of a [MapLibre GL JS example](https://maplibre.org/maplibre-gl-js/docs/examples/) with maplibre-rs compiled to WebAssembly.

maplibre-rs is configured by a [style document](https://maplibre.org/maplibre-style-spec/) alone. Where GL JS calls `addSource` and `addLayer` once the style has loaded, an example adds the same source and layer to the document before the map starts. Each example module exports a function returning the document, which the embedded page passes to `startMapLibre`:

```js
import { startMapLibre } from 'maplibre-rs'
import style from './display-a-map.js'

await startMapLibre(undefined, undefined, JSON.stringify(await style()))
```

Natively, the demo opens a document saved as JSON:

```bash
cargo run -p maplibre-demo -- headed --style style.json
```

DOM controls, popups and event handlers of the GL JS examples are left out.

## Getting Started

- [Display a Map](./display-a-map.md)
- [Display a Satellite Map](./display-a-satellite-map.md)

## Globe

- [Display a Globe with a Vector Map](./display-a-globe-with-a-vector-map.md)
- [Display a Globe with an Atmosphere](./display-a-globe-with-an-atmosphere.md)

## Terrain & Hillshade

- [Add 3D Terrain](./add-3d-terrain.md)
- [Add a Hillshade Layer](./add-a-hillshade-layer.md)

## 3D Models & Buildings

- [Display Buildings in 3D](./display-buildings-in-3d.md)

## Vector & GeoJSON

- [Add a GeoJSON Line](./add-a-geojson-line.md)
- [Add a GeoJSON Polygon](./add-a-geojson-polygon.md)

## Raster & Imagery

- [Add a Raster Tile Source](./add-a-raster-tile-source.md)

## Expressions

- [Style Lines with a Data-Driven Property](./style-lines-with-a-data-driven-property.md)

## Heatmaps & Clusters

- [Create a Heatmap Layer](./create-a-heatmap-layer.md)
- [Create and Style Clusters](./create-and-style-clusters.md)
