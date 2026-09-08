#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[test]
fn prefetch_limits_preserve_every_primary_tile() {
    let tiles: Vec<_> = (0..200)
        .map(|x| WorldTileCoords {
            x,
            y: 0,
            z: ZoomLevel::new(8),
        })
        .collect();
    let primary = ViewRegion::from_tiles(tiles.clone(), ZoomLevel::new(8), tiles.len());
    let secondary = ViewRegion::from_tiles(
        vec![WorldTileCoords {
            x: 200,
            y: 0,
            z: ZoomLevel::new(8),
        }],
        ZoomLevel::new(8),
        1,
    );
    let combined =
        union_regions(Some(primary), Some(secondary), ZoomLevel::new(8), 96).expect("a region");
    assert_eq!(combined.iter().collect::<Vec<_>>(), tiles);
}

#[test]
fn moving_eye_requests_start_with_all_visible_tiles() {
    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        render::{
            camera::EyeFrustum,
            view_state::{ExternalAnchor, ExternalView},
        },
        window::PhysicalSize,
    };
    use cgmath::{Deg, Matrix4, Rad, SquareMatrix, Vector3};
    let style: Style = serde_json::from_str(
        r#"{"version":8,"sources":{},"layers":[],"terrain":{"source":"dem"}}"#,
    )
    .expect("style");
    let mut world = World::default();
    for pitch in [60.0, 85.0, 90.0] {
        let mut view = ViewState::new(
            PhysicalSize::new(3840, 2160).expect("viewport"),
            WorldCoords::from((256.0, 256.0)),
            Zoom::new(10.0),
            Deg(0.0),
            Deg(45.0),
        );
        let mut destination = None;
        for height in [8000.0, 4000.0] {
            view.set_external_view(
                ExternalView {
                    anchor: ExternalAnchor {
                        position: LatLon::new(47.26, 11.39),
                        altitude_meters: 600.0,
                    },
                    view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, height))
                        * Matrix4::from_angle_x(Deg(pitch)))
                    .invert()
                    .expect("eye"),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 16.0 / 9.0, 0.05, 1.0e8),
                },
                &ProjectionType::Mercator,
            )
            .expect("pose");
            if destination.is_none() {
                destination = Some(view.clone());
            }
        }
        assert!(!view.eye_settled());
        view.set_request_overscan(1.5);
        assert_visible_requests(&style, &view, &world);
        world.resources.insert(PrefetchView {
            view_state: destination,
            placement: None,
        });
        assert_visible_requests(&style, &view, &world);
        world.resources.insert(PrefetchView::default());
    }
}

fn assert_visible_requests(style: &Style, view: &ViewState, world: &World) {
    let level = view.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let visible = view_region_for_projection(style, view, world, level, ViewStatePadding::Tight)
        .expect("visible covering")
        .expect("visible region");
    let requests = view_region_for_projection(style, view, world, level, ViewStatePadding::Loose)
        .expect("request covering")
        .expect("request region");
    let visible: Vec<_> = visible.iter().collect();
    assert!(!visible.is_empty());
    assert_eq!(
        requests.iter().take(visible.len()).collect::<Vec<_>>(),
        visible
    );
}
