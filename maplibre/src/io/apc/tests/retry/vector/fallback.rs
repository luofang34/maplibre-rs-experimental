//! A child with only one completed source cannot replace complete parent pixels.
use super::{
    super::{fixture::Fixture, source::Response},
    deliver, manual,
    render::{assert_green, Frames},
    tile,
};
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom},
    io::tile_sources::TileKind,
    render::{
        eventually::Eventually, tile_view_pattern::WgpuTileViewPattern, view_state::ViewState,
    },
    vector::VectorLayerBucketComponent,
};

fn drawn_sources(test: &Fixture) -> Vec<WorldTileCoords> {
    let Some(Eventually::Initialized(pattern)) = test
        .context
        .world
        .resources
        .get::<Eventually<WgpuTileViewPattern>>()
    else {
        panic!("pattern");
    };
    let mut sources = Vec::new();
    for view in pattern.iter() {
        view.render_kind(TileKind::Vector, |shape| sources.push(shape.coords()));
    }
    sources
}

async fn parent() -> (Fixture, Frames) {
    let (mut test, mut frames) = manual(true).await;
    test.source.set_healthy(Response::Bytes(tile()));
    deliver(&mut test, Response::Bytes(tile())).await;
    assert_green(&frames.render(&mut test));
    for source in test.context.style.sources.values_mut() {
        if let crate::style::source::Source::Vector(source) = source {
            source.maxzoom = Some(1);
        }
    }
    test.context.view_state = ViewState::new(
        crate::window::PhysicalSize::new(16, 16).expect("size"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(1.0),
        cgmath::Deg(0.0),
        cgmath::Rad(0.64),
    );
    (test, frames)
}

mod tests;
