//! Layer specification collection and raster-aware target selection for terrain draping.

use crate::{
    coords::WorldTileCoords,
    io::tile_sources::RasterSourceId,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        tile_view_pattern::RasterCoverings,
    },
    style::Style,
    tcs::world::World,
    terrain::drape_targets::{
        is_drapeable, select_targets_as_with, Arrivals, ShapeSource, ShapeSpec, TargetSpec,
        VectorLayerSpec,
    },
    vector::VectorBufferPool,
};
#[cfg(feature = "raster")]
use crate::{
    hillshade::dem_layer_kind, io::tile_sources::RASTER_LAYER_TYPES,
    raster::resource::RasterResources,
};

/// Pairs every view tile with the source tiles that hold its data, checking raster missing state.
pub(crate) fn select_targets(
    coords: impl Iterator<Item = WorldTileCoords>,
    world: &World,
    raster_coverings: &RasterCoverings,
) -> Vec<(WorldTileCoords, Vec<ShapeSource>)> {
    select_targets_as(coords, world, raster_coverings, Arrivals::Together)
}

/// [`select_targets`] with the say over how a loading raster set shows.
pub(crate) fn select_targets_as(
    coords: impl Iterator<Item = WorldTileCoords>,
    world: &World,
    raster_coverings: &RasterCoverings,
    arrivals: Arrivals,
) -> Vec<(WorldTileCoords, Vec<ShapeSource>)> {
    select_targets_as_with(
        coords,
        world,
        raster_coverings,
        arrivals,
        &source_tile_missing,
    )
}

fn source_tile_missing(source: &RasterSourceId, coords: WorldTileCoords, world: &World) -> bool {
    #[cfg(feature = "raster")]
    {
        world
            .tiles
            .query::<&crate::raster::RasterLayersDataComponent>(coords)
            .is_some_and(|component| component.source_is_missing(source))
    }
    #[cfg(not(feature = "raster"))]
    {
        let _ = (source, coords, world);
        false
    }
}

/// Lists the drapeable layers each source tile currently holds.
pub(crate) fn collect_layer_specs(
    targets: Vec<(WorldTileCoords, Vec<ShapeSource>)>,
    style: &Style,
    world: &World,
    zoom: f64,
) -> Vec<TargetSpec> {
    let vector = world.resources.get::<Eventually<VectorBufferPool>>();
    #[cfg(feature = "raster")]
    let raster = world.resources.get::<Eventually<RasterResources>>();
    #[cfg(feature = "raster")]
    let raster_layers = visible_raster_layers(style, raster, zoom);
    #[cfg(not(feature = "raster"))]
    let _ = style;
    targets
        .into_iter()
        .map(|(coords, shapes)| {
            #[cfg(feature = "raster")]
            let raster_ctx = (raster, raster_layers.as_slice());
            #[cfg(not(feature = "raster"))]
            let raster_ctx = ();
            build_target_spec(coords, shapes, vector, raster_ctx, zoom)
        })
        .collect()
}

#[cfg(feature = "raster")]
type RasterLayerRef = (String, u32, RasterSourceId, bool);

#[cfg(feature = "raster")]
fn visible_raster_layers(
    style: &Style,
    raster: Option<&Eventually<RasterResources>>,
    zoom: f64,
) -> Vec<RasterLayerRef> {
    style
        .layers
        .iter()
        .filter(|layer| {
            RASTER_LAYER_TYPES.contains(&layer.type_.as_str()) && layer.is_visible_at(zoom)
        })
        .filter_map(|layer| {
            let Some(Initialized(resources)) = raster else {
                return None;
            };
            Some((
                layer.id.clone(),
                layer.index,
                resources.layer_source(&layer.id)?.clone(),
                dem_layer_kind(&layer.type_).is_some(),
            ))
        })
        .collect()
}

#[cfg(feature = "raster")]
type RasterCtx<'a> = (
    Option<&'a Eventually<RasterResources>>,
    &'a [RasterLayerRef],
);
#[cfg(not(feature = "raster"))]
type RasterCtx<'a> = ();

#[cfg(feature = "raster")]
fn shape_raster_layers(
    shape: &ShapeSource,
    (raster, raster_layers): RasterCtx<'_>,
) -> Vec<(String, u32, bool)> {
    if raster_layers.is_empty() {
        return Vec::new();
    }
    raster_layers
        .iter()
        .filter(|(_, _, source, _)| shape.raster_source.as_ref() == Some(source))
        .filter(|(id, _, _, _)| {
            matches!(
                raster,
                Some(Initialized(resources)) if resources.layer_texture(id, &shape.coords).is_some()
            )
        })
        .map(|(id, index, _, dem)| (id.clone(), *index, *dem))
        .collect()
}

#[cfg(not(feature = "raster"))]
fn shape_raster_layers(
    _shape: &ShapeSource,
    _raster_ctx: RasterCtx<'_>,
) -> Vec<(String, u32, bool)> {
    Vec::new()
}

fn build_target_spec(
    coords: WorldTileCoords,
    shapes: Vec<ShapeSource>,
    vector: Option<&Eventually<VectorBufferPool>>,
    raster_ctx: RasterCtx<'_>,
    zoom: f64,
) -> TargetSpec {
    let absent_sources = if shapes.iter().any(|shape| shape.absent) {
        shapes
            .iter()
            .filter(|shape| shape.absent)
            .filter_map(|shape| shape.raster_source.clone())
            .collect()
    } else {
        Vec::new()
    };
    let specs = shapes
        .into_iter()
        .filter(|shape| !shape.absent)
        .map(|shape| ShapeSpec {
            view_complete: shape.view_complete,
            source: shape.coords,
            is_vector: shape.raster_source.is_none(),
            vector_layers: match (&shape.raster_source, vector) {
                (None, Some(Initialized(pool))) => vector_layer_specs(pool, shape.coords, zoom),
                _ => Vec::new(),
            },
            raster_layers: shape_raster_layers(&shape, raster_ctx),
        })
        .collect();
    TargetSpec {
        coords,
        shapes: specs,
        absent_sources,
    }
}

fn vector_layer_specs(
    pool: &VectorBufferPool,
    source: WorldTileCoords,
    zoom: f64,
) -> Vec<VectorLayerSpec> {
    let Some(entries) = pool.as_ref().get_layers(source) else {
        return Vec::new();
    };
    let mut specs = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.style_layer.is_visible_at(zoom)
            && is_drapeable(&entry.style_layer.type_)
            && crate::vector::structures::kind(&entry.style_layer).is_none()
        {
            specs.push(VectorLayerSpec {
                id: entry.style_layer_id,
                index: entry.style_layer.index,
                is_line: entry.style_layer.type_ == "line",
                coords: entry.coords,
                content_revision: entry.content_revision(),
            });
        }
    }
    specs
}

#[cfg(all(test, feature = "raster"))]
#[expect(
    clippy::expect_used,
    reason = "unit tests unwrap fixture parsing and tile setup"
)]
mod tests {
    use std::{collections::HashSet, sync::Arc};

    use image::{Rgba, RgbaImage};

    use super::*;
    use crate::{
        io::{
            apc::{IntoMessage, Message, MessageTag},
            tile_sources::TileKind,
        },
        raster::{
            AvailableRasterLayerData, DefaultRasterTransferables, LayerRaster, RasterSourceId,
            RasterTransferables,
        },
        render::{
            settings::BufferPoolSizes,
            tile_view_pattern::{HasTile, ViewTileSources},
        },
        terrain::{
            transferables::{DemMessageTag, LayerDem},
            DemTile,
        },
    };

    #[derive(Default)]
    struct LoadedTiles(HashSet<WorldTileCoords>);

    impl HasTile for LoadedTiles {
        fn has_tile(&self, coords: WorldTileCoords, _world: &World) -> bool {
            self.0.contains(&coords)
        }
    }

    #[test]
    fn missing_children_wait_for_sparse_fallback_ancestor_before_settling() {
        let ancestor = WorldTileCoords::from((75, 96, 8_u8.into()));
        let target = WorldTileCoords::from((300, 384, 10_u8.into()));
        let children = target.get_children();

        let mut world = World::default();
        let mut sources = ViewTileSources::default();
        sources.add_resource_query::<&LoadedTiles>(TileKind::Raster);
        world.resources.insert(sources);
        world.resources.insert(LoadedTiles(HashSet::new()));

        let src = RasterSourceId::from("vfr_sectional");
        for child in &children {
            let mut comp = crate::raster::RasterLayersDataComponent::default();
            comp.record(crate::raster::RasterLayerData::Missing(
                crate::raster::MissingRasterLayerData {
                    coords: *child,
                    source: src.clone(),
                },
            ));
            world.tiles.spawn_mut(*child).expect("spawn").insert(comp);
        }

        let pending_shapes: Vec<_> = select_targets(
            std::iter::once(target),
            &world,
            &[(src.clone(), children.to_vec())],
        )
        .pop()
        .expect("target")
        .1
        .into_iter()
        .filter(|shape| shape.raster_source.is_some())
        .collect();
        assert!(
            pending_shapes.is_empty(),
            "must wait for fallback walk, got {pending_shapes:?}"
        );

        for z9_or_z10 in [target, target.get_parent().expect("z9")] {
            let mut comp = crate::raster::RasterLayersDataComponent::default();
            comp.record(crate::raster::RasterLayerData::Missing(
                crate::raster::MissingRasterLayerData {
                    coords: z9_or_z10,
                    source: src.clone(),
                },
            ));
            world
                .tiles
                .spawn_mut(z9_or_z10)
                .expect("spawn")
                .insert(comp);
        }
        world
            .resources
            .insert(LoadedTiles([ancestor].into_iter().collect()));
        let settled_shapes: Vec<_> =
            select_targets(std::iter::once(target), &world, &[(src, children.to_vec())])
                .pop()
                .expect("target")
                .1
                .into_iter()
                .filter(|shape| shape.raster_source.is_some())
                .collect();
        assert_eq!(settled_shapes.len(), 1);
        assert_eq!(settled_shapes[0].coords, ancestor);
        assert!(settled_shapes[0].view_complete);
        assert!(!settled_shapes[0].absent);
    }

    const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];

    fn terrarium_pixel(elevation: f64) -> Rgba<u8> {
        let value = elevation + 32768.0;
        let red = (value / 256.0).floor();
        let green = (value - red * 256.0).floor();
        let blue = ((value - red * 256.0 - green) * 256.0).round();
        Rgba([red as u8, green as u8, blue as u8, 255])
    }

    fn gradient_tile(base: f64) -> DemTile {
        let mut image = RgbaImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                image.put_pixel(
                    x,
                    y,
                    terrarium_pixel(base + 100.0 * f64::from(x) + 10.0 * f64::from(y)),
                );
            }
        }
        DemTile::from_image(&image, TERRARIUM).expect("square image decodes")
    }

    #[derive(Debug)]
    struct SerializedDem(WorldTileCoords, RgbaImage);
    impl IntoMessage for SerializedDem {
        fn into(self) -> Message {
            Message::new(Self::message_tag(), Box::new(self))
        }
    }
    impl LayerDem for SerializedDem {
        fn message_tag() -> &'static dyn MessageTag {
            &DemMessageTag::LayerDem
        }
        fn build_from(coords: WorldTileCoords, image: RgbaImage) -> Self {
            Self(coords, image)
        }
        fn coords(&self) -> WorldTileCoords {
            self.0
        }
        fn into_image(self) -> RgbaImage {
            self.1
        }
    }

    #[derive(Debug)]
    struct SerializedRaster(WorldTileCoords, RasterSourceId, RgbaImage);
    impl IntoMessage for SerializedRaster {
        fn into(self) -> Message {
            Message::new(Self::message_tag(), Box::new(self))
        }
    }
    impl LayerRaster for SerializedRaster {
        fn message_tag() -> &'static dyn MessageTag {
            <DefaultRasterTransferables as RasterTransferables>::LayerRaster::message_tag()
        }
        fn build_from(coords: WorldTileCoords, source: RasterSourceId, image: RgbaImage) -> Self {
            Self(coords, source, image)
        }
        fn coords(&self) -> WorldTileCoords {
            self.0
        }
        fn to_layer(self) -> AvailableRasterLayerData {
            AvailableRasterLayerData {
                coords: self.0,
                source: self.1,
                image: self.2,
            }
        }
    }

    #[test]
    fn to_rgba_image_and_serialized_transferables_roundtrip_interior_samples() {
        let mut source_image = RgbaImage::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                source_image.put_pixel(
                    x,
                    y,
                    terrarium_pixel(250.0 + 75.0 * f64::from(x) + 12.5 * f64::from(y)),
                );
            }
        }
        let mut dem = DemTile::from_image(&source_image, TERRARIUM).expect("DEM");
        let neighbour = gradient_tile(4000.0);
        dem.backfill_border(&neighbour, 1, 0).expect("border");

        let reconstructed = dem.to_rgba_image();
        assert_eq!(reconstructed, source_image);

        let shared = Arc::new(dem);
        let empty_fallback = Arc::new(RgbaImage::new(0, 0));
        let coords = WorldTileCoords::from((132, 91, 8_u8.into()));

        let dem_msg = SerializedDem::build_from_cached_dem(
            coords,
            Arc::clone(&shared),
            40_075_016.686,
            Arc::clone(&empty_fallback),
        );
        assert_eq!(dem_msg.into_image(), source_image);

        let raster_msg = SerializedRaster::build_from_dem(
            coords,
            RasterSourceId::from("terrain-dem"),
            shared,
            40_075_016.686,
            empty_fallback,
        );
        assert_eq!(raster_msg.to_layer().image, source_image);

        let defaults = BufferPoolSizes::default();
        assert!(defaults.feature_metadata >= 3_000_000);
        assert!(defaults.feature_metadata < defaults.vertices);
    }
}
