//! Spatial and linear tile query indexes crossing the worker FlatBuffer boundary.
use maplibre::io::geometry_index::wire::{WireGeometry, WireTileIndex};

use super::*;

impl LayerIndexed for FlatBufferTransferable {
    fn message_tag() -> &'static dyn MessageTag {
        &WebMessageTag::LayerIndexed
    }

    fn build_from(coords: WorldTileCoords, source: Option<String>, index: TileIndex) -> Self {
        let wire = WireTileIndex::from_tile_index(&index);
        let estimated = 256
            + wire.prop_blobs.len()
            + wire.prop_offsets.len() * 4
            + wire.geometries.len() * std::mem::size_of::<FlatIndexedGeometry>()
            + wire.ring_ends.len() * 4
            + wire.xy.len() * 8;
        let mut inner_builder = FlatBufferBuilder::with_capacity(estimated);

        let source_fb = source.as_deref().map(|s| inner_builder.create_string(s));
        let string_offsets: Vec<_> = wire
            .strings
            .iter()
            .map(|s| inner_builder.create_string(s))
            .collect();
        let strings_fb = inner_builder.create_vector(&string_offsets);
        let prop_blobs_fb = inner_builder.create_vector(&wire.prop_blobs);
        let prop_offsets_fb = inner_builder.create_vector(&wire.prop_offsets);
        let flat_geoms: Vec<FlatIndexedGeometry> = wire
            .geometries
            .iter()
            .map(|g| {
                FlatIndexedGeometry::new(
                    g.id,
                    g.source_layer_idx,
                    g.prop_group_idx,
                    g.feature_index,
                    g.ring_start,
                    g.ring_count,
                    g.coord_start,
                    g.coord_count,
                    g.kind,
                    g.has_id,
                )
            })
            .collect();
        let geometries_fb = inner_builder.create_vector(&flat_geoms);
        let ring_ends_fb = inner_builder.create_vector(&wire.ring_ends);
        let xy_fb = inner_builder.create_vector(&wire.xy);

        let mut builder = FlatLayerIndexedBuilder::new(&mut inner_builder);
        builder.add_coords(&FlatWorldTileCoords::new(
            coords.x,
            coords.y,
            coords.z.into(),
        ));
        if let Some(source_fb) = source_fb {
            builder.add_source(source_fb);
        }
        builder.add_is_spatial(wire.is_spatial);
        builder.add_strings(strings_fb);
        builder.add_prop_blobs(prop_blobs_fb);
        builder.add_prop_offsets(prop_offsets_fb);
        builder.add_geometries(geometries_fb);
        builder.add_ring_ends(ring_ends_fb);
        builder.add_xy(xy_fb);

        let root = builder.finish();
        inner_builder.finish(root, None);
        let (data, start) = inner_builder.collapse();
        FlatBufferTransferable {
            tag: WebMessageTag::LayerIndexed,
            data,
            start,
        }
    }

    fn coords(&self) -> WorldTileCoords {
        root_as_flat_layer_indexed(&self.data[self.start..])
            .ok()
            .and_then(|data| data.coords())
            .map_or_else(WorldTileCoords::default, Into::into)
    }

    fn source(&self) -> Option<String> {
        root_as_flat_layer_indexed(&self.data[self.start..])
            .ok()
            .and_then(|data| data.source())
            .map(str::to_owned)
    }

    fn to_tile_index(self) -> TileIndex {
        let Ok(data) = root_as_flat_layer_indexed(&self.data[self.start..]) else {
            return WireTileIndex {
                is_spatial: false,
                strings: Vec::new(),
                prop_blobs: Vec::new(),
                prop_offsets: Vec::new(),
                geometries: Vec::new(),
                ring_ends: Vec::new(),
                xy: Vec::new(),
            }
            .into_tile_index();
        };
        let strings = data
            .strings()
            .map(|v| v.iter().map(str::to_owned).collect())
            .unwrap_or_default();
        let prop_blobs = data
            .prop_blobs()
            .map(|v| v.bytes().to_vec())
            .unwrap_or_default();
        let prop_offsets = data
            .prop_offsets()
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        let geometries = data
            .geometries()
            .map(|v| {
                v.iter()
                    .map(|g| WireGeometry {
                        id: g.id(),
                        source_layer_idx: g.source_layer_idx(),
                        prop_group_idx: g.prop_group_idx(),
                        feature_index: g.feature_index(),
                        ring_start: g.ring_start(),
                        ring_count: g.ring_count(),
                        coord_start: g.coord_start(),
                        coord_count: g.coord_count(),
                        kind: g.kind(),
                        has_id: g.has_id(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ring_ends = data
            .ring_ends()
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        let xy = data.xy().map(|v| v.iter().collect()).unwrap_or_default();

        WireTileIndex {
            is_spatial: data.is_spatial(),
            strings,
            prop_blobs,
            prop_offsets,
            geometries,
            ring_ends,
            xy,
        }
        .into_tile_index()
    }
}

#[cfg(test)]
mod tests {
    use maplibre::{
        coords::{InnerCoords, WorldTileCoords, ZoomLevel},
        io::geometry_index::wire::{WireGeometry, WireTileIndex},
        vector::LayerIndexed,
    };
    use wasm_bindgen_test::*;

    use super::FlatBufferTransferable;

    #[wasm_bindgen_test]
    fn flatbuffer_layer_indexed_roundtrips_spatial_and_source() {
        assert!(<FlatBufferTransferable as LayerIndexed>::requires_geometry_index());
        let wire = WireTileIndex {
            is_spatial: true,
            strings: vec!["road".into()],
            prop_blobs: vec![0, 0, 0, 0],
            prop_offsets: vec![0, 4],
            geometries: vec![WireGeometry {
                id: 99,
                source_layer_idx: 0,
                prop_group_idx: 0,
                feature_index: 3,
                ring_start: 0,
                ring_count: 0,
                coord_start: 0,
                coord_count: 2,
                kind: 1,
                has_id: true,
            }],
            ring_ends: vec![],
            xy: vec![100.0, 200.0, 300.0, 200.0],
        };
        let coords = WorldTileCoords {
            x: 4,
            y: 5,
            z: ZoomLevel::new(3),
        };
        let transferable = <FlatBufferTransferable as LayerIndexed>::build_from(
            coords,
            Some("openmaptiles".into()),
            wire.into_tile_index(),
        );
        assert_eq!(LayerIndexed::coords(&transferable), coords);
        assert_eq!(
            LayerIndexed::source(&transferable).as_deref(),
            Some("openmaptiles")
        );
        let decoded = transferable.to_tile_index();
        let hits = decoded.point_query(InnerCoords { x: 200.0, y: 202.0 });
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Some(99));
        assert_eq!(&*hits[0].source_layer, "road");
    }
}
