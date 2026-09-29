//! Replacements invalidate only the drapes that draw the changed geometry.
use super::super::fixture::{Fixture, Kind};
use super::render::Frames;
use crate::{
    sdf::SymbolLayersDataComponent,
    terrain::{DemTile, DemTileComponent, LoadedDem},
    vector::VectorLayerBucketComponent,
};

async fn fixture() -> (Fixture, Frames) {
    let mut test = Fixture::new(Kind::Vector, false).await;
    test.context.style.sources.insert(
        "dem".into(),
        serde_json::from_value(serde_json::json!({
            "type":"raster-dem", "tiles":["offline://dem"], "encoding":"terrarium", "maxzoom":0
        }))
        .expect("DEM source"),
    );
    test.context.style.terrain =
        Some(serde_json::from_value(serde_json::json!({"source":"dem"})).expect("terrain"));
    let dem = DemTile::from_image(
        &image::RgbaImage::from_pixel(8, 8, image::Rgba([128, 0, 0, 255])),
        [256.0, 1.0, 1.0 / 256.0, 32768.0],
    )
    .expect("flat DEM");
    test.context
        .world
        .tiles
        .spawn_mut(Default::default())
        .expect("tile")
        .insert(DemTileComponent::Loaded(LoadedDem::new(dem)))
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    let frames = Frames::new(&mut test);
    (test, frames)
}

fn redraws(test: &Fixture) -> usize {
    test.context
        .world
        .resources
        .get::<crate::terrain::DrapePhase>()
        .expect("drape phase")
        .targets
        .len()
}

fn unrelated_upload(test: &mut Fixture) {
    use crate::{
        coords::WorldTileCoords,
        render::{
            eventually::Eventually,
            shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
        },
        vector::{VectorBufferPool, VectorLayerBucket},
    };
    let world = &mut test.context.world;
    let component = world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("source");
    let VectorLayerBucket::AvailableLayer(bucket) = &component.layers[0] else {
        panic!("geometry");
    };
    let Some(Eventually::Initialized(pool)) =
        world.resources.get_mut::<Eventually<VectorBufferPool>>()
    else {
        panic!("pool");
    };
    pool.replace_layer_geometry(
        &test.context.renderer.queue,
        WorldTileCoords::from((9, 0, 4_u8.into())),
        test.context.style.layers[0].clone(),
        &bucket.buffer,
        ShaderLayerMetadata::new(0.0, 0.0, [0.0; 2]),
        &vec![FillShaderFeatureMetadata { color: [1.0; 4] }; bucket.buffer.buffer.vertices.len()],
    )
    .expect("unrelated allocation");
}

mod tests;
