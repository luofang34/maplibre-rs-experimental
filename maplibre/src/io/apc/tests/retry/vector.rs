//! Vector tile loading through the production request and reply systems.

fn tile() -> Vec<u8> {
    use geozero::mvt::{tile, Message, Tile};
    Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "land".into(),
            features: vec![tile::Feature {
                id: Some(1),
                tags: vec![],
                r#type: Some(tile::GeomType::Polygon as i32),
                geometry: vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15],
            }],
            keys: vec![],
            values: vec![],
            extent: Some(4096),
        }],
    }
    .encode_to_vec()
}

mod render;
mod rendered_query;
mod tests;

async fn manual(multiple: bool) -> (super::fixture::Fixture, render::Frames) {
    use crate::{sdf::SymbolLayersDataComponent, vector::VectorLayerBucketComponent};
    let mut test = super::fixture::Fixture::new(super::fixture::Kind::Vector, multiple).await;
    let frames = render::Frames::new(&mut test);
    test.context
        .world
        .tiles
        .spawn_mut(Default::default())
        .expect("tile")
        .insert(VectorLayerBucketComponent::default())
        .insert(SymbolLayersDataComponent::default());
    (test, frames)
}

async fn deliver(test: &mut super::fixture::Fixture, response: super::source::Response) {
    use crate::io::apc::{AsyncProcedureCall, Input};
    test.source.set(response);
    test.kernel
        .apc()
        .call(
            Input::TileRequest {
                coords: Default::default(),
                style: test.context.style.clone(),
            },
            crate::vector::request_system::fetch_vector_apc::<
                _,
                crate::vector::DefaultVectorTransferables,
                _,
            >,
        )
        .expect("worker admitted");
    test.receive().await;
}

mod replacement;

mod classification;

mod stale;

mod terrain;

mod fallback;
