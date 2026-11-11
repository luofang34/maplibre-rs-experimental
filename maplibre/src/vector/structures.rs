//! Opt-in terrain-relative road profiles for sources without a surveyed vertical alignment.
//!
//! A style opts in through `metadata["maplibre-rs:terrain-structure"]` (`bridge` or
//! `tunnel`). This is renderer metadata, not a MapLibre line paint property. The profile
//! joins sampled endpoints; the configured clearance is an inference, never an OSM `layer`
//! converted to metres. A supplied absolute deck elevation overrides that inference.
//!
//! `overlay` instead keeps a line on the terrain surface at its style width in screen pixels,
//! where a draped line has the ground width of that many pixels at the center of the view.
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use super::{
    tessellation::{
        unpack_datum_pair, IndexDataType, LineElevationOptions, OverAlignedVertexBuffer,
    },
    AvailableVectorLayerBucket, VectorBufferPool,
};
use crate::{
    coords::WorldTileCoords,
    render::ShaderVertex,
    style::{
        layer::{ElevationDatum, LayerPaint, StyleLayer},
        Style,
    },
    tcs::{tiles::Tiles, world::World},
    terrain::{coverage::TerrainCoverageIndex, source::dem_source, DemTileComponent},
};

const MAX_VERTICES: usize = 524_288;
pub(crate) type SpatialBuffer = (
    WorldTileCoords,
    String,
    OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StructureKind {
    Bridge,
    Tunnel,
    /// Stands on the terrain surface, as a heatmap's points do.
    Surface,
    /// Stands on the ground the terrain draws under its centre, as a GL JS circle samples the
    /// DEM of the terrain tile beneath it.
    Marker,
    /// Rises from the terrain or sea level according to its base and height datums.
    Extrusion,
    /// Lies on the terrain surface at the width its style gives in screen pixels, wherever on
    /// the screen it is; the shader finds the surface under each vertex.
    Overlay,
    /// Lifts stroked line geometry to an altitude (`line-elevation`) in metres while keeping its
    /// screen-pixel width.
    ElevatedLine,
}

pub(crate) fn kind(layer: &StyleLayer) -> Option<StructureKind> {
    if layer.type_ == "heatmap" {
        return Some(StructureKind::Surface);
    }
    if layer.type_ == "circle" {
        return Some(StructureKind::Marker);
    }
    if layer.type_ == "fill-extrusion" {
        return Some(StructureKind::Extrusion);
    }
    if layer.type_ != "line" {
        return None;
    }
    if matches!(&layer.paint, Some(LayerPaint::Line(paint)) if paint.line_elevation.is_some()) {
        return Some(StructureKind::ElevatedLine);
    }
    match layer
        .metadata
        .as_ref()?
        .get("maplibre-rs:terrain-structure")?
        .as_str()?
    {
        "bridge" => Some(StructureKind::Bridge),
        "tunnel" => Some(StructureKind::Tunnel),
        "overlay" => Some(StructureKind::Overlay),
        _ => None,
    }
}

/// Whether a layer is drawn over the terrain surface rather than draped into its textures.
pub(crate) fn is_overlay(layer: &StyleLayer) -> bool {
    matches!(kind(layer), Some(StructureKind::Overlay))
}

#[derive(Default)]
struct CachedProfiles {
    key: u64,
    buffers: Arc<Vec<SpatialBuffer>>,
}

pub(crate) fn for_frame(
    world: &mut World,
    style: &Style,
    sources: &[WorldTileCoords],
) -> (Arc<Vec<SpatialBuffer>>, bool) {
    if dem_source(style).is_none() {
        if let Some(cached) = world.resources.get::<CachedProfiles>() {
            if cached.key == 0 {
                return (cached.buffers.clone(), false);
            }
        }
        let buffers = Arc::new(Vec::new());
        world.resources.insert(CachedProfiles {
            key: 0,
            buffers: buffers.clone(),
        });
        return (buffers, false);
    }
    // The terrain this frame draws, which markers stand on.
    let drawn = world.resources.get::<TerrainCoverageIndex>();
    let key = fingerprint(&world.tiles, style, sources, drawn);
    if let Some(cached) = world.resources.get::<CachedProfiles>() {
        if cached.key == key {
            return (cached.buffers.clone(), false);
        }
    }
    let buffers = Arc::new(prepare(&world.tiles, style, sources, drawn));
    world.resources.insert(CachedProfiles {
        key,
        buffers: buffers.clone(),
    });
    (buffers, true)
}

fn fingerprint(
    tiles: &Tiles,
    style: &Style,
    sources: &[WorldTileCoords],
    drawn: Option<&TerrainCoverageIndex>,
) -> u64 {
    let mut hash = std::hash::DefaultHasher::new();
    // A marker moves when the drawn terrain switches DEM tiles, even if none has loaded. The
    // index is rebuilt every frame in no fixed order, so its tiles are sorted first.
    let has_markers = style
        .layers
        .iter()
        .any(|layer| matches!(kind(layer), Some(StructureKind::Marker)));
    if let Some(drawn) = drawn.filter(|_| has_markers) {
        let mut terrain: Vec<_> = drawn
            .rendered_tiles()
            .map(|coords| (coords, drawn.loaded_dem_for(coords)))
            .collect();
        terrain.sort_unstable();
        terrain.hash(&mut hash);
    }
    style
        .terrain
        .as_ref()
        .map(|terrain| terrain.exaggeration.to_bits())
        .hash(&mut hash);
    for coords in tiles
        .pyramid_of::<DemTileComponent>()
        .coords_coarse_to_fine()
    {
        if let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(coords) {
            coords.hash(&mut hash);
            dem.revision.hash(&mut hash);
            dem.tile.min().to_bits().hash(&mut hash);
            dem.tile.max().to_bits().hash(&mut hash);
        }
    }
    for coords in sources {
        coords.hash(&mut hash);
        if let Some(pending) = tiles.query::<&super::content::LayerReplacements>(*coords) {
            pending.revision.hash(&mut hash);
        }
        for bucket in super::content::vector_layers(tiles, *coords) {
            bucket.style_layer_id.hash(&mut hash);
            bucket.buffer.buffer.vertices.len().hash(&mut hash);
            if let Some(first) = bucket.buffer.buffer.vertices.first() {
                first.corner_code.to_bits().hash(&mut hash);
                first.elevation.to_bits().hash(&mut hash);
                first.distance.to_bits().hash(&mut hash);
            }
        }
    }
    for layer in &style.layers {
        layer.id.hash(&mut hash);
        kind(layer).hash(&mut hash);
        if let Some(metadata) = &layer.metadata {
            let mut entries: Vec<_> = metadata.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            entries.hash(&mut hash);
        }
    }
    hash.finish()
}

fn is_pure_msl_bucket(kind: StructureKind, vertices: &[ShaderVertex]) -> bool {
    match kind {
        StructureKind::Extrusion => vertices.iter().all(|v| {
            unpack_datum_pair(v.corner_code) == (ElevationDatum::Msl, ElevationDatum::Msl)
        }),
        StructureKind::ElevatedLine => vertices
            .iter()
            .all(|v| LineElevationOptions::unpack_datum(v.corner_code) == ElevationDatum::Msl),
        _ => false,
    }
}

fn prepare(
    tiles: &Tiles,
    style: &Style,
    sources: &[WorldTileCoords],
    drawn: Option<&TerrainCoverageIndex>,
) -> Vec<SpatialBuffer> {
    let Some(dem) = dem_source(style) else {
        return Vec::new();
    };
    let index = TerrainCoverageIndex::build(sources.iter().copied(), tiles, &dem);
    let mut output = Vec::new();
    let mut remaining = MAX_VERTICES;
    for coords in sources {
        for bucket in super::content::vector_layers(tiles, *coords) {
            let Some(layer) = style.layers.iter().find(|l| l.id == bucket.style_layer_id) else {
                continue;
            };
            // An overlay finds the ground on the GPU, so its geometry stays as tessellated.
            let Some(kind) = kind(layer).filter(|kind| !matches!(kind, StructureKind::Overlay))
            else {
                continue;
            };
            let count = bucket.buffer.buffer.vertices.len();
            if count == 0
                || count > remaining
                || is_pure_msl_bucket(kind, &bucket.buffer.buffer.vertices)
            {
                continue;
            }
            let mut buffer = OverAlignedVertexBuffer::from_iters(
                bucket.buffer.buffer.vertices.iter().copied(),
                bucket.buffer.buffer.indices.iter().copied(),
                bucket.buffer.usable_indices,
            );
            let clearance = number(layer, "maplibre-rs:structure-clearance-meters")
                .unwrap_or(6.0)
                .clamp(0.0, 100.0);
            let absolute = number(layer, "maplibre-rs:structure-elevation-meters");
            elevate(
                &mut buffer.buffer.vertices,
                bucket,
                kind,
                clearance,
                absolute,
                |position| {
                    let scale = 2_f64.powi(i32::from(u8::from(coords.z)));
                    let x = (f64::from(coords.x) + f64::from(position[0]) / 4096.0) / scale;
                    let y = (f64::from(coords.y) + f64::from(position[1]) / 4096.0) / scale;
                    let finest = || {
                        index.loaded_ground_cached(tiles, x, y, crate::terrain::DemDetail::Finest)
                    };
                    match kind {
                        StructureKind::Marker => drawn
                            .and_then(|drawn| drawn.elevation_at(tiles, x, y))
                            .or_else(finest),
                        _ => finest(),
                    }
                },
            );
            remaining -= count;
            output.push((*coords, layer.id.clone(), buffer));
        }
    }
    output
}

fn number(layer: &StyleLayer, key: &str) -> Option<f64> {
    let value = layer.metadata.as_ref()?.get(key)?;
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

fn elevate(
    vertices: &mut [ShaderVertex],
    bucket: &AvailableVectorLayerBucket,
    kind: StructureKind,
    clearance: f64,
    absolute: Option<f64>,
    sample: impl Fn([f32; 2]) -> Option<f64>,
) {
    let mut start = 0;
    for count in &bucket.feature_indices {
        let end = (start + *count as usize).min(vertices.len());
        if let Some(feature) = vertices.get_mut(start..end) {
            elevate_span(feature, kind, clearance, absolute, &sample);
        }
        start = end;
    }
}

fn centroid_ground(
    vertices: &[ShaderVertex],
    sample: &impl Fn([f32; 2]) -> Option<f64>,
) -> Option<f32> {
    let count = vertices.len().max(1) as f32;
    let middle = vertices.iter().fold([0.0_f32; 2], |sum, vertex| {
        [sum[0] + vertex.position[0], sum[1] + vertex.position[1]]
    });
    sample([middle[0] / count, middle[1] / count]).map(|ground| ground as f32)
}

fn elevate_span(
    vertices: &mut [ShaderVertex],
    kind: StructureKind,
    clearance: f64,
    absolute: Option<f64>,
    sample: &impl Fn([f32; 2]) -> Option<f64>,
) {
    let Some(first_vertex) = vertices.first() else {
        return;
    };
    if matches!(kind, StructureKind::Extrusion) {
        let (base_datum, height_datum) = unpack_datum_pair(first_vertex.corner_code);
        if (base_datum, height_datum) == (ElevationDatum::Msl, ElevationDatum::Msl) {
            return;
        }
        let middle_ground = if matches!(base_datum, ElevationDatum::CentroidAgl)
            || matches!(height_datum, ElevationDatum::CentroidAgl)
        {
            centroid_ground(vertices, sample).unwrap_or(0.0)
        } else {
            0.0
        };
        let needs_vertex = matches!(base_datum, ElevationDatum::VertexAgl)
            || matches!(height_datum, ElevationDatum::VertexAgl);
        for vertex in vertices {
            let vertex_ground = if needs_vertex {
                sample(vertex.position).unwrap_or(0.0) as f32
            } else {
                0.0
            };
            let offset = |datum| match datum {
                ElevationDatum::Msl => 0.0,
                ElevationDatum::CentroidAgl => middle_ground,
                ElevationDatum::VertexAgl => vertex_ground,
            };
            vertex.distance += offset(base_datum);
            vertex.elevation += offset(height_datum);
            if base_datum != height_datum {
                vertex.elevation = vertex.elevation.max(vertex.distance);
            }
        }
        return;
    }
    if matches!(kind, StructureKind::ElevatedLine) {
        match LineElevationOptions::unpack_datum(first_vertex.corner_code) {
            ElevationDatum::Msl => {}
            ElevationDatum::CentroidAgl => {
                if let Some(ground) = centroid_ground(vertices, sample) {
                    for vertex in vertices {
                        vertex.elevation += ground;
                    }
                }
            }
            ElevationDatum::VertexAgl => {
                for vertex in vertices {
                    if let Some(ground) = sample(vertex.position) {
                        vertex.elevation += ground as f32;
                    }
                }
            }
        }
        return;
    }
    if matches!(kind, StructureKind::Surface | StructureKind::Marker) {
        for vertex in vertices {
            if let Some(ground) = sample(vertex.position) {
                vertex.elevation = ground as f32;
            }
        }
        return;
    }
    let Some(first) = vertices
        .iter()
        .min_by(|a, b| a.distance.total_cmp(&b.distance))
        .copied()
    else {
        return;
    };
    let Some(last) = vertices
        .iter()
        .max_by(|a, b| a.distance.total_cmp(&b.distance))
        .copied()
    else {
        return;
    };
    let length = f64::from(last.distance - first.distance);
    if !length.is_finite() || length <= 1e-6 {
        return;
    }
    let (Some(a), Some(b)) = (sample(first.position), sample(last.position)) else {
        return;
    };
    for vertex in vertices {
        let t = (f64::from(vertex.distance - first.distance) / length).clamp(0.0, 1.0);
        let ground = sample(vertex.position).unwrap_or(a + (b - a) * t);
        vertex.elevation =
            absolute.unwrap_or_else(|| profile(kind, a, b, ground, t, clearance)) as f32;
    }
}

fn profile(kind: StructureKind, start: f64, end: f64, ground: f64, t: f64, clearance: f64) -> f64 {
    let chord = start + (end - start) * t;
    let transition = (t * 8.0).min((1.0 - t) * 8.0).clamp(0.0, 1.0);
    let separation = clearance * transition * transition * (3.0 - 2.0 * transition);
    match kind {
        StructureKind::Bridge => chord.max(ground + separation) + 0.5,
        StructureKind::Tunnel => chord.min(ground - separation) + 0.1,
        StructureKind::Surface
        | StructureKind::Marker
        | StructureKind::Extrusion
        | StructureKind::Overlay
        | StructureKind::ElevatedLine => ground,
    }
}

pub(crate) fn refresh_gpu(
    pool: &VectorBufferPool,
    queue: &crate::render::upload_queue::UploadQueue,
    buffers: &[SpatialBuffer],
    tiles: &Tiles,
) {
    for (coords, id, buffer) in buffers {
        // Pending geometry must not overwrite an allocation whose indices and paint are retained.
        if tiles
            .query::<&super::content::LayerReplacements>(*coords)
            .is_some_and(|pending| {
                pending
                    .vector
                    .iter()
                    .any(|layer| layer.style_layer_id == *id)
            })
        {
            continue;
        }
        if let Some(entry) = pool
            .index()
            .get_layers(*coords)
            .and_then(|layers| layers.iter().find(|entry| entry.style_layer.id == *id))
        {
            pool.update_vertices(queue, entry, &buffer.buffer.vertices);
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
mod pixels;
