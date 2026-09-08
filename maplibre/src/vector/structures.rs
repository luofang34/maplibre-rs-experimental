//! Opt-in terrain-relative road profiles for sources without a surveyed vertical alignment.
//!
//! A style opts in through `metadata["maplibre-rs:terrain-structure"]` (`bridge` or
//! `tunnel`). This is renderer metadata, not a MapLibre line paint property. The profile
//! joins sampled endpoints; the configured clearance is an inference, never an OSM `layer`
//! converted to metres. A supplied absolute deck elevation overrides that inference.
use super::{
    tessellation::{IndexDataType, OverAlignedVertexBuffer},
    AvailableVectorLayerBucket, VectorBufferPool, VectorLayerBucket, VectorLayerBucketComponent,
};
use crate::{
    coords::WorldTileCoords,
    render::ShaderVertex,
    style::{layer::StyleLayer, Style},
    tcs::{tiles::Tiles, world::World},
    terrain::{coverage::TerrainCoverageIndex, source::dem_source, DemTileComponent},
};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

const MAX_VERTICES: usize = 65_536;
pub(crate) type SpatialBuffer = (
    WorldTileCoords,
    String,
    OverAlignedVertexBuffer<ShaderVertex, IndexDataType>,
);

#[derive(Clone, Copy)]
pub(crate) enum StructureKind {
    Bridge,
    Tunnel,
}

pub(crate) fn kind(layer: &StyleLayer) -> Option<StructureKind> {
    if layer.type_ != "line" {
        return None;
    }
    match layer
        .metadata
        .as_ref()?
        .get("maplibre-rs:terrain-structure")?
        .as_str()
    {
        "bridge" => Some(StructureKind::Bridge),
        "tunnel" => Some(StructureKind::Tunnel),
        _ => None,
    }
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
    let key = fingerprint(&world.tiles, style, sources);
    if let Some(cached) = world.resources.get::<CachedProfiles>() {
        if cached.key == key {
            return (cached.buffers.clone(), false);
        }
    }
    let buffers = Arc::new(prepare(&world.tiles, style, sources));
    world.resources.insert(CachedProfiles {
        key,
        buffers: buffers.clone(),
    });
    (buffers, true)
}

fn fingerprint(tiles: &Tiles, style: &Style, sources: &[WorldTileCoords]) -> u64 {
    let mut hash = std::hash::DefaultHasher::new();
    style
        .terrain
        .as_ref()
        .map(|terrain| terrain.exaggeration.to_bits())
        .hash(&mut hash);
    for tile in tiles.tiles.values() {
        if let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(tile.coords) {
            tile.coords.hash(&mut hash);
            dem.revision.hash(&mut hash);
            dem.tile.min().to_bits().hash(&mut hash);
            dem.tile.max().to_bits().hash(&mut hash);
        }
    }
    for coords in sources {
        coords.hash(&mut hash);
        if let Some(buckets) = tiles.query::<&VectorLayerBucketComponent>(*coords) {
            for bucket in &buckets.layers {
                if let VectorLayerBucket::AvailableLayer(bucket) = bucket {
                    bucket.buffer.buffer.vertices.len().hash(&mut hash);
                }
            }
        }
    }
    for layer in &style.layers {
        if let Some(metadata) = &layer.metadata {
            let mut entries: Vec<_> = metadata.iter().collect();
            entries.sort();
            entries.hash(&mut hash);
        }
    }
    hash.finish()
}

fn prepare(tiles: &Tiles, style: &Style, sources: &[WorldTileCoords]) -> Vec<SpatialBuffer> {
    let Some(dem) = dem_source(style) else {
        return Vec::new();
    };
    let index = TerrainCoverageIndex::build(sources.iter().copied(), tiles, &dem);
    let mut output = Vec::new();
    let mut remaining = MAX_VERTICES;
    for coords in sources {
        let Some(buckets) = tiles.query::<&VectorLayerBucketComponent>(*coords) else {
            continue;
        };
        for bucket in &buckets.layers {
            let VectorLayerBucket::AvailableLayer(bucket) = bucket else {
                continue;
            };
            let Some(layer) = style.layers.iter().find(|l| l.id == bucket.style_layer_id) else {
                continue;
            };
            let Some(kind) = kind(layer) else {
                continue;
            };
            let count = bucket.buffer.buffer.vertices.len();
            if count == 0 || count > remaining {
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
                    index.elevation_at_zoom(
                        tiles,
                        (f64::from(coords.x) + f64::from(position[0]) / 4096.0) / scale,
                        (f64::from(coords.y) + f64::from(position[1]) / 4096.0) / scale,
                        u8::from(coords.z),
                    )
                },
            );
            remaining -= count;
            output.push((*coords, layer.id.clone(), buffer));
        }
    }
    output
}

fn number(layer: &StyleLayer, key: &str) -> Option<f64> {
    layer
        .metadata
        .as_ref()?
        .get(key)?
        .parse::<f64>()
        .ok()
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

fn elevate_span(
    vertices: &mut [ShaderVertex],
    kind: StructureKind,
    clearance: f64,
    absolute: Option<f64>,
    sample: &impl Fn([f32; 2]) -> Option<f64>,
) {
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
    }
}

pub(crate) fn refresh_gpu(pool: &VectorBufferPool, queue: &wgpu::Queue, buffers: &[SpatialBuffer]) {
    for (coords, id, buffer) in buffers {
        if let Some(entry) = pool
            .index()
            .get_layers(*coords)
            .and_then(|layers| layers.iter().find(|entry| entry.style_layer.id == *id))
        {
            let bytes = bytemuck::cast_slice(&buffer.buffer.vertices);
            let range = entry.vertices_buffer_range();
            if bytes.len() as u64 == range.end - range.start {
                queue.write_buffer(pool.vertices(), range.start, bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
mod pixels;
