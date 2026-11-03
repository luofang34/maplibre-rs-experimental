//! Polar cap fill colours for globe terrain tiles.
//!
//! Terrain polar caps never stretch a drape texture's edge row to the pole, avoiding radial
//! streaks from coastlines or raster imagery. Instead, each cap composites any draped vector
//! `fill` layers whose tessellated geometry explicitly extends to that pole (`NORTH_POLE_Y` or
//! `SOUTH_POLE_Y`) over the style's background clear colour.

use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    projection::globe::tile_mesh::{NORTH_POLE_Y, SOUTH_POLE_Y},
    style::{
        layer::{LayerPaint, StyleLayer},
        Style,
    },
    tcs::tiles::Tiles,
    terrain::queue_system::drape_phase::background_clear_color,
    vector::{
        upload_system::paint::uniform_color, AvailableVectorLayerBucket, VectorLayerBucket,
        VectorLayerBucketComponent,
    },
};

/// Returns `[north_rgba8, south_rgba8]` packed as little-endian `0xAABBGGRR` for `coords`.
pub(super) fn cap_colors(
    coords: WorldTileCoords,
    drape_source: Option<WorldTileCoords>,
    style: &Style,
    tiles: &Tiles,
    zoom: f64,
) -> [u32; 2] {
    let clear = background_clear_color(style);
    let base = [
        clear.r as f32,
        clear.g as f32,
        clear.b as f32,
        clear.a as f32,
    ];
    let packed_base = pack_rgba8(base);
    if drape_source.is_none() {
        return [packed_base, packed_base];
    }

    let z = u8::from(coords.z);
    let max_row = (1_i32.checked_shl(u32::from(z)).unwrap_or(1)) - 1;
    let north = if coords.y == 0 {
        pole_color(coords, true, base, style, tiles, zoom)
    } else {
        packed_base
    };
    let south = if coords.y == max_row {
        pole_color(coords, false, base, style, tiles, zoom)
    } else {
        packed_base
    };
    [north, south]
}

fn pole_color(
    coords: WorldTileCoords,
    north: bool,
    base: [f32; 4],
    style: &Style,
    tiles: &Tiles,
    zoom: f64,
) -> u32 {
    let Some(buckets) = find_pole_buckets(coords, north, tiles) else {
        return pack_rgba8(base);
    };
    let pole_y = if north {
        f32::from(NORTH_POLE_Y)
    } else {
        f32::from(SOUTH_POLE_Y)
    };
    let mut acc = base;
    for layer in &style.layers {
        if layer.is_hidden()
            || layer.type_ != "fill"
            || crate::vector::structures::kind(layer).is_some()
        {
            continue;
        }
        if matches!(
            layer.paint.as_ref(),
            Some(LayerPaint::Fill(paint)) if paint.fill_pattern.is_some()
        ) {
            continue;
        }
        let Some(bucket) = buckets.layers.iter().find_map(|entry| match entry {
            VectorLayerBucket::AvailableLayer(bucket) if bucket.style_layer_id == layer.id => {
                Some(bucket)
            }
            _ => None,
        }) else {
            continue;
        };
        if let Some(src) = pole_feature_color(layer, bucket, pole_y, zoom) {
            let inv = 1.0 - src[3];
            acc = [
                src[0] * src[3] + acc[0] * inv,
                src[1] * src[3] + acc[1] * inv,
                src[2] * src[3] + acc[2] * inv,
                src[3] + acc[3] * inv,
            ];
        }
    }
    pack_rgba8(acc)
}

fn find_pole_buckets(
    coords: WorldTileCoords,
    north: bool,
    tiles: &Tiles,
) -> Option<&VectorLayerBucketComponent> {
    let mut probe = Some(coords);
    while let Some(tile) = probe {
        if let Some(component) = tiles.query::<&VectorLayerBucketComponent>(tile) {
            if component
                .layers
                .iter()
                .any(|entry| matches!(entry, VectorLayerBucket::AvailableLayer(_)))
            {
                return Some(component);
            }
        }
        probe = tile.get_parent();
    }
    if u8::from(coords.z) == 0 {
        let child = WorldTileCoords {
            x: 0,
            y: i32::from(!north),
            z: ZoomLevel::new(1),
        };
        if let Some(component) = tiles.query::<&VectorLayerBucketComponent>(child) {
            if component
                .layers
                .iter()
                .any(|entry| matches!(entry, VectorLayerBucket::AvailableLayer(_)))
            {
                return Some(component);
            }
        }
    }
    None
}

fn pole_feature_color(
    layer: &StyleLayer,
    bucket: &AvailableVectorLayerBucket,
    pole_y: f32,
    zoom: f64,
) -> Option<[f32; 4]> {
    let vertices = &bucket.buffer.buffer.vertices;
    let uniform = uniform_color(layer, zoom);
    let fallback: [f32; 4] = layer
        .paint
        .as_ref()
        .and_then(|paint| paint.get_color())
        .map(Into::into)
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let mut start = 0_usize;
    let mut matched = None;
    for (index, &count) in bucket.feature_indices.iter().enumerate() {
        let end = start.saturating_add(count as usize).min(vertices.len());
        if vertices[start..end]
            .iter()
            .any(|vertex| vertex.position[1] == pole_y)
        {
            matched = Some(uniform.unwrap_or_else(|| {
                bucket
                    .feature_colors
                    .get(index)
                    .copied()
                    .unwrap_or(fallback)
            }));
        }
        start = end;
    }
    matched
}

fn pack_rgba8(color: [f32; 4]) -> u32 {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u32;
    byte(color[0]) | (byte(color[1]) << 8) | (byte(color[2]) << 16) | (byte(color[3]) << 24)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use lyon::tessellation::VertexBuffers;

    use super::*;
    use crate::{render::ShaderVertex, vector::tessellation::OverAlignedVertexBuffer};

    fn unpack(packed: u32) -> [u8; 4] {
        [
            (packed & 0xff) as u8,
            ((packed >> 8) & 0xff) as u8,
            ((packed >> 16) & 0xff) as u8,
            ((packed >> 24) & 0xff) as u8,
        ]
    }

    #[test]
    fn polar_cap_composites_pole_fill_only_when_draped() {
        let style: Style = serde_json::from_str(
            r##"{
                "version": 8,
                "sources": {},
                "layers": [
                    {"id": "bg", "type": "background", "paint": {"background-color": "#ffffff"}},
                    {"id": "water", "type": "fill", "source": "v", "paint": {"fill-color": "#0000ff", "fill-opacity": 0.5}}
                ]
            }"##,
        )
        .expect("style");
        let root = WorldTileCoords::default();
        let mut raw = VertexBuffers::new();
        raw.vertices.push(ShaderVertex::new(
            [0.0, f32::from(NORTH_POLE_Y)],
            [0.0, 0.0],
        ));
        raw.vertices.push(ShaderVertex::new(
            [4096.0, f32::from(NORTH_POLE_Y)],
            [0.0, 0.0],
        ));
        raw.indices.extend([0_u32, 0, 1]);
        let bucket = AvailableVectorLayerBucket {
            coords: root,
            source_layer: "water".into(),
            style_layer_id: "water".into(),
            buffer: OverAlignedVertexBuffer::from_iters(
                raw.vertices,
                raw.indices.iter().copied(),
                raw.indices.len() as u32,
            ),
            feature_indices: vec![1, 1],
            feature_colors: vec![],
            feature_sort_keys: vec![],
        };
        let mut tiles = Tiles::default();
        tiles
            .spawn_mut(root)
            .expect("tile")
            .insert(VectorLayerBucketComponent {
                done: true,
                failed: false,
                overscaled_zoom: 0,
                layers: vec![VectorLayerBucket::AvailableLayer(bucket)],
            });

        let undraped = cap_colors(root, None, &style, &tiles, 0.0);
        assert_eq!(unpack(undraped[0]), [255, 255, 255, 255]);
        assert_eq!(unpack(undraped[1]), [255, 255, 255, 255]);

        let draped = cap_colors(root, Some(root), &style, &tiles, 0.0);
        assert_eq!(unpack(draped[0]), [128, 128, 255, 255]);
        assert_eq!(unpack(draped[1]), [255, 255, 255, 255]);
    }
}
