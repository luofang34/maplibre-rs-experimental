# Terrain rendering contract

Surface paint uses one view zoom for every source level. Terrain quantizes that zoom to one eighth of a level for cache reuse. The bundled palette keeps landcover colour and opacity constant across zooms; feature detail can increase without changing the base palette. A drape is ready only when every source layer is uploaded. A fallback uses complete children or an available ancestor. Coarsened budget targets are requested explicitly and receive upload priority.

The globe background uses relative far depth, so it cannot cover a valid distant terrain fragment. Terrain includes polar fans beyond the Mercator boundary, samples the boundary texture, and closes at a shared zero-height pole. Polar lighting uses the geometric fan positions rather than collapsed texture coordinates. DEM tiles do not provide surveyed polar elevation.

Place labels enter by geographic hierarchy, with smaller and fainter local names. Collision padding and line spacing control density. Viewport-aligned glyphs keep readable proportions; projected text shorter than seven pixels in an external view is omitted before collision placement and selection queries. MSAA, SDF edge smoothing and terrain drape mipmaps remain enabled.

## Transport elevation

The default style preserves OpenMapTiles transportation filters and OSM crossing order. Bridge and tunnel lines opt into depth-tested terrain profiles through renderer metadata:

- `maplibre-rs:terrain-structure`: `bridge` or `tunnel`.
- `maplibre-rs:structure-clearance-meters`: assumed ground separation, default 6 metres.
- `maplibre-rs:structure-elevation-meters`: optional absolute elevation in the DEM datum.

Metadata values are strings. Styles without this opt-in retain ordinary MapLibre cartographic line rendering. This extension does not introduce an unsupported standard paint property. A bridge joins sampled endpoints and clears the intervening ground. A tunnel follows an inferred buried profile and is occluded by terrain. Profile geometry is cached and bounded to 65,536 vertices.

These are visual approximations from tile-clipped centerlines. They do not supply bridge piers, deck thickness, portal excavation or a surveyed vertical alignment. Complete structural assembly needs connected spans, deck outlines, elevations and crossing constraints from a richer source. The renderer must not interpret an OSM layer number as metres. Absolute elevation metadata can override an inferred profile when appropriate source information exists.

The [MapLibre style specification](https://maplibre.org/maplibre-style-spec/layers/) defines line styling but does not define full bridge or tunnel structural assembly. [OpenMapTiles transportation](https://github.com/openmaptiles/openmaptiles/blob/master/layers/transportation/transportation.yaml) supplies `brunnel` and `layer`; [OSM layer semantics](https://wiki.openstreetmap.org/wiki/Key:layer) describe relative order. [OSM2World bridge generation](https://github.com/tordanik/OSM2World/blob/master/core/src/main/java/org/osm2world/world/modules/BridgeModule.java) provides a reference for deck, underside and pier geometry when connected source data is available.
