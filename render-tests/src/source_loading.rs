//! Decodes the fixture sources selected by the renderer's visible tile coverage.

use std::collections::HashMap;

use maplibre::{
    coords::WorldTileCoords,
    geojson::index::GeoJsonIndex,
    headless::map::{
        process_geojson_layers_with_atlas, process_tile_layers_with_atlas, HeadlessMap,
        ProcessedLayers,
    },
    io::tile_sources::{tile_in_bounds, MAX_OVERZOOMING},
    projection::ProjectionType,
    raster::AvailableRasterLayerData,
    style::{
        layer::{LayerPaint, StyleLayer},
        source::{
            GeoJsonData, GeoJsonSource, Source, VectorSource, GEOJSON_DEFAULT_MAXZOOM,
            GEOJSON_LAYER,
        },
        Style,
    },
    terrain::dem_tile_coords,
};
use serde_json::Value;

use crate::{
    image_sources::PlacedImage,
    paths::{local_data_path, local_tile_path},
    source_tiles::source_tile_coords,
    symbol_assets::load_atlas_blocking,
    vector_feature_state::VectorFeatureStates,
};

/// The tiles each raster source is fading out after a zoom, with the opacity they keep.
pub(super) type DepartingRasterTiles = HashMap<String, (Vec<WorldTileCoords>, f32)>;

pub(super) fn load_sources_blocking(
    map: &mut HeadlessMap,
    style: &Style,
    (view_coords, paused, departing): (
        &[WorldTileCoords],
        &HashMap<String, Vec<WorldTileCoords>>,
        &DepartingRasterTiles,
    ),
    (images, pixel_ratio, vector_states): (
        &HashMap<String, PlacedImage>,
        f64,
        &VectorFeatureStates,
    ),
) -> Result<(ProcessedLayers, Vec<AvailableRasterLayerData>), String> {
    let mut all_layers = ProcessedLayers::default();
    let mut all_raster_layers = Vec::new();
    let projection = style
        .projection
        .as_ref()
        .map_or_else(ProjectionType::default, |specification| {
            specification.projection_type.clone()
        });
    for (name, source) in &style.sources {
        let layers: Vec<_> = style
            .layers
            .iter()
            .filter(|layer| layer.source.as_deref() == Some(name.as_str()))
            .cloned()
            .collect();
        if layers.is_empty() {
            continue;
        }
        let target_coords = paused.get(name).map_or(view_coords, Vec::as_slice);
        match source {
            Source::GeoJson(source) => all_layers.append(&mut load_geojson_blocking(
                map,
                (style, name, source),
                &layers,
                target_coords,
                (&projection, pixel_ratio, paused.contains_key(name)),
            )?),
            Source::Vector(source) => all_layers.append(&mut load_vector_blocking(
                map,
                (style, name, source),
                &layers,
                target_coords,
                (&projection, pixel_ratio),
                vector_states,
            )?),
            // DEM images supply hillshade and colour relief independently from terrain meshes.
            Source::Image(_) => return Err(format!("Image source '{name}' was not lowered")),
            Source::Raster(_) | Source::RasterDem(_) => {
                all_raster_layers.extend(match images.get(name) {
                    Some(placed) => load_image_blocking(map, name, placed)?,
                    None => load_raster_blocking(
                        map,
                        (name, source),
                        departing.get(name).map_or(&[][..], |(tiles, _)| tiles),
                    )?,
                })
            }
        }
    }
    Ok((all_layers, all_raster_layers))
}

fn load_geojson_blocking(
    map: &mut HeadlessMap,
    (style, name, source): (&Style, &str, &GeoJsonSource),
    layers: &[StyleLayer],
    target_coords: &[WorldTileCoords],
    (projection, pixel_ratio, paused): (&ProjectionType, f64, bool),
) -> Result<ProcessedLayers, String> {
    let data = &source.data;
    let loaded;
    let value = match data {
        GeoJsonData::Inline(value) => value.as_ref(),
        GeoJsonData::Url(url) => {
            let path = local_data_path(url)?;
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("Cannot read GeoJSON {}: {error}", path.display()))?;
            loaded = serde_json::from_str::<Value>(&text)
                .map_err(|error| format!("Cannot parse GeoJSON {}: {error}", path.display()))?;
            &loaded
        }
    };
    // A document that is not GeoJSON is reported and leaves the source empty, as in GL JS.
    let index = match GeoJsonIndex::from_value(value, source) {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!(source = name, %error, "GeoJSON source is not usable and draws nothing");
            return Ok(ProcessedLayers::default());
        }
    };
    // The glyphs a tile needs are found in the same tile the worker path would build, whose
    // one layer has a fixed name that the style's layers do not carry.
    let mut symbol_style = style.clone();
    for layer in &mut symbol_style.layers {
        if layer.source.as_deref() == Some(name) {
            layer.source_layer = Some(GEOJSON_LAYER.to_owned());
        }
    }
    let mut processed = ProcessedLayers::default();
    // Past its last zoom a GeoJSON source is drawn magnified from the tile of that zoom.
    let max_zoom = source.maxzoom.unwrap_or(GEOJSON_DEFAULT_MAXZOOM);
    let level = u8::from(
        map.view_state()
            .zoom()
            .zoom_level(maplibre::render::tile_view_pattern::DEFAULT_TILE_SIZE),
    );
    let mut tiles: Vec<WorldTileCoords> = Vec::new();
    for coords in target_coords {
        let coords = maplibre::io::tile_sources::clamp_to_max_zoom(*coords, Some(max_zoom));
        if !tiles.contains(&coords) {
            tiles.push(coords);
        }
    }
    for coords in &tiles {
        // A paused source keeps its tiles while the view moves on, so they are laid out at the
        // zoom the view has.
        let magnified = if paused || u8::from(coords.z) >= max_zoom {
            level.max(u8::from(coords.z))
        } else {
            0
        };
        let atlas = symbol_atlas(
            &symbol_style,
            layers,
            &index.tile(*coords),
            (*coords, 0, pixel_ratio),
        )?;
        // A source that filters or clusters shows each tile its own version of the document.
        let presented = index.source_document(value, source, *coords);
        processed.append(
            &mut process_geojson_layers_with_atlas(
                presented.as_ref().unwrap_or(value),
                name,
                layers.to_vec(),
                *coords,
                projection.clone(),
                (atlas, magnified, Some(source.buffer.unwrap_or(128))),
            )
            .map_err(|error| format!("Cannot process GeoJSON source '{name}': {error}"))?,
        );
    }
    Ok(processed)
}

fn load_vector_blocking(
    map: &HeadlessMap,
    (style, name, source): (&Style, &str, &VectorSource),
    layers: &[StyleLayer],
    target_coords: &[WorldTileCoords],
    (projection, pixel_ratio): (&ProjectionType, f64),
    vector_states: &VectorFeatureStates,
) -> Result<ProcessedLayers, String> {
    let template = source
        .tiles
        .as_ref()
        .and_then(|templates| templates.first())
        .ok_or_else(|| format!("Vector source '{name}' has no tile template"))?;
    let mut processed = ProcessedLayers::default();
    for coords in source_tile_coords(target_coords, 0, source.minzoom, source.maxzoom) {
        // Tiles outside the source's declared bounds are never requested.
        if source
            .bounds
            .is_some_and(|bounds| !tile_in_bounds(coords, bounds))
        {
            continue;
        }
        let path = local_tile_path(template, coords, source.scheme)?;
        // Missing fixture tiles behave as 404 responses and keep pyramid fallback eligible.
        let data = match std::fs::read(&path) {
            Ok(data) => data.into_boxed_slice(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "vector tile unavailable");
                continue;
            }
        };
        let data = if vector_states.has(name) {
            vector_states.apply(name, &data)?.into_boxed_slice()
        } else {
            data
        };
        let magnified = overscaled_zoom(map, style, source, coords);
        let atlas = symbol_atlas(style, layers, &data, (coords, magnified, pixel_ratio))?;
        for layer in layers {
            processed.append(
                &mut process_tile_layers_with_atlas(
                    &data,
                    layer,
                    coords,
                    projection.clone(),
                    (atlas.clone(), magnified),
                )
                .map_err(|error| format!("Cannot process vector source '{name}': {error}"))?,
            );
        }
    }
    Ok(processed)
}

/// The zoom a source tile is magnified to at the view's zoom level once the view is past the
/// source's last zoom, which only matters to layers laid out along lines.
fn overscaled_zoom(
    map: &HeadlessMap,
    style: &Style,
    source: &VectorSource,
    coords: WorldTileCoords,
) -> u8 {
    let level = u8::from(
        map.view_state()
            .zoom()
            .zoom_level(maplibre::render::tile_view_pattern::DEFAULT_TILE_SIZE),
    );
    match source.maxzoom {
        Some(max_zoom)
            if maplibre::vector::depends_on_overscaling(style)
                && u8::from(coords.z) >= max_zoom =>
        {
            level.max(u8::from(coords.z))
        }
        _ => 0,
    }
}

fn load_image_blocking(
    map: &HeadlessMap,
    name: &str,
    placed: &PlacedImage,
) -> Result<Vec<AvailableRasterLayerData>, String> {
    let required = map
        .required_raster_tile_coords(name)
        .map_err(|error| format!("Cannot select raster tiles: {error}"))?;
    Ok(required
        .into_iter()
        .map(|coords| AvailableRasterLayerData {
            coords,
            source: name.into(),
            // A tile the image does not touch is empty, which is still a tile that has loaded.
            image: maplibre::raster::image_source::render_tile(
                &placed.image,
                placed.coordinates,
                coords,
            )
            .unwrap_or_else(|| image::RgbaImage::new(1, 1)),
        })
        .collect())
}

/// Reads the raster tiles the view needs, and the `departing` ones a zoom left fading out.
fn load_raster_blocking(
    map: &HeadlessMap,
    (name, source): (&str, &Source),
    departing: &[WorldTileCoords],
) -> Result<Vec<AvailableRasterLayerData>, String> {
    let (template, minzoom, scheme) = match source {
        Source::Raster(source) => (
            source.tiles.as_ref().and_then(|tiles| tiles.first()),
            source.minzoom,
            source.scheme,
        ),
        Source::RasterDem(source) => (
            source.tiles.as_ref().and_then(|tiles| tiles.first()),
            source.minzoom,
            None,
        ),
        _ => (None, None, None),
    };
    let template =
        template.ok_or_else(|| format!("Raster source '{name}' has no tile template"))?;
    let minzoom = minzoom.unwrap_or(0);
    let mut required = map
        .required_raster_tile_coords(name)
        .map_err(|error| format!("Cannot select raster tiles: {error}"))?;
    required.extend_from_slice(departing);
    let mut seen = std::collections::BTreeSet::new();
    let mut layers = Vec::new();
    for ideal in required {
        let mut coords = ideal;
        loop {
            if !seen.insert(coords) {
                break;
            }
            let path = local_tile_path(template, coords, scheme)?;
            match image::open(&path) {
                Ok(image) => {
                    layers.push(AvailableRasterLayerData {
                        coords,
                        source: name.into(),
                        image: image.to_rgba8(),
                    });
                    break;
                }
                Err(error) => tracing::debug!(path = %path.display(), %error, "no raster tile"),
            }
            // The nearest fixture ancestor stands in for GL JS's fallback after a 404.
            match coords.get_parent() {
                Some(parent)
                    if u8::from(parent.z) >= minzoom
                        && u8::from(ideal.z) - u8::from(parent.z) <= MAX_OVERZOOMING =>
                {
                    coords = parent
                }
                _ => break,
            }
        }
    }
    Ok(layers)
}

/// Reads the DEM tiles the terrain needs for the target tiles from the local asset tree.
///
/// Fixtures only ship the tiles their own view needs, so a tile that is missing on disk is
/// skipped and the mesh falls back to an ancestor or to sea level.
pub(super) fn load_dem_tiles_blocking(
    style: &Style,
    target_coords: &[WorldTileCoords],
) -> Result<Vec<(WorldTileCoords, image::RgbaImage)>, String> {
    let Some(terrain) = &style.terrain else {
        return Ok(Vec::new());
    };
    let Some(Source::RasterDem(dem)) = style.sources.get(&terrain.source) else {
        return Err(format!(
            "Terrain source '{}' is not a raster-dem source",
            terrain.source
        ));
    };
    let Some(template) = dem.tiles.as_ref().and_then(|templates| templates.first()) else {
        return Err(format!(
            "Terrain source '{}' has no tile template",
            terrain.source
        ));
    };
    let minzoom = dem.minzoom.unwrap_or(0);
    let maxzoom = dem.maxzoom.unwrap_or(22);
    let mut seen = std::collections::BTreeSet::new();
    let mut tiles = Vec::new();
    for coords in target_coords {
        let Some(ideal) = dem_tile_coords(*coords, minzoom, maxzoom) else {
            continue;
        };
        // A tile the fixture does not ship answers 404 in GL JS, which then loads the parent;
        // the nearest ancestor on disk stands in the same way.
        let mut coords = ideal;
        loop {
            if !seen.insert(coords) {
                break;
            }
            let path = local_tile_path(template, coords, None)?;
            match image::open(&path) {
                Ok(image) => {
                    tiles.push((coords, image.to_rgba8()));
                    break;
                }
                Err(error) => {
                    tracing::debug!(%coords, path = %path.display(), %error, "no DEM tile");
                }
            }
            match coords.get_parent() {
                Some(parent)
                    if u8::from(parent.z) >= minzoom
                        && u8::from(ideal.z) - u8::from(parent.z) <= MAX_OVERZOOMING =>
                {
                    coords = parent;
                }
                _ => break,
            }
        }
    }
    Ok(tiles)
}

/// The atlas for the symbol layers among `layers`, or `None` when there are none.
fn symbol_atlas(
    style: &Style,
    layers: &[StyleLayer],
    tile: &[u8],
    (coords, magnified, pixel_ratio): (WorldTileCoords, u8, f64),
) -> Result<Option<std::sync::Arc<maplibre::sdf::assets::SymbolAtlas>>, String> {
    if !layers
        .iter()
        .any(|layer| matches!(layer.paint, Some(LayerPaint::Symbol(_))))
    {
        return Ok(None);
    }
    load_atlas_blocking(
        style,
        tile,
        (f64::from(magnified.max(u8::from(coords.z))), pixel_ratio),
    )
}
