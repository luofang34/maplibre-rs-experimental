use crate::{coords::WorldTileCoords, tcs::system::SystemError};

#[test]
fn tile_and_readiness_failures_are_recoverable() {
    assert!(SystemError::InvalidTile {
        coords: WorldTileCoords::default()
    }
    .is_recoverable());
    assert!(SystemError::Dependencies.is_recoverable());
}

#[test]
fn renderer_failures_stop_the_map() {
    assert!(!SystemError::Setup.is_recoverable());
    let graph = crate::render::error::RenderError::GraphRun(Box::new(std::fmt::Error));
    assert!(!SystemError::Render(graph).is_recoverable());
}
