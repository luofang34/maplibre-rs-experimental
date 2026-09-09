#![allow(clippy::expect_used, clippy::panic)]

use super::{dem_ancestor_coords, dem_tile_coords, missing_dem_fallback};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    tcs::world::World,
    terrain::DemTileComponent,
};

fn tile(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn dem_tile_sits_one_zoom_level_above_the_view_tile() {
    assert_eq!(
        dem_tile_coords(tile(2201, 1453, 12), 0, 12),
        Some(tile(1100, 726, 11))
    );
    assert_eq!(dem_tile_coords(tile(0, 0, 0), 0, 12), Some(tile(0, 0, 0)));
}

#[test]
fn dem_tile_is_clamped_to_the_source_zoom_range() {
    assert_eq!(
        dem_tile_coords(tile(8804, 5812, 14), 0, 12),
        Some(tile(2201, 1453, 12))
    );
    assert_eq!(dem_tile_coords(tile(3, 2, 3), 5, 12), None);
}

#[test]
fn a_coarse_ancestor_accompanies_every_dem_tile_for_culling() {
    assert_eq!(
        dem_ancestor_coords(tile(1100, 726, 11), 0),
        Some(tile(17, 11, 5))
    );
    assert_eq!(dem_ancestor_coords(tile(17, 11, 5), 0), None);
    assert_eq!(dem_ancestor_coords(tile(3, 2, 3), 0), None);
    assert_eq!(
        dem_ancestor_coords(tile(1100, 726, 11), 8),
        Some(tile(137, 90, 8)),
        "the ancestor never drops below the source minimum zoom"
    );
}

#[test]
fn a_missing_dem_tile_falls_back_to_the_nearest_ancestor_that_may_exist() {
    let mut world = World::default();
    let ideal = tile(2201, 1453, 12);
    let parent = tile(1100, 726, 11);
    let grandparent = tile(550, 363, 10);
    for coords in [ideal, parent] {
        world
            .tiles
            .spawn_mut(coords)
            .expect("valid coordinates")
            .insert(DemTileComponent::Missing);
    }

    assert_eq!(
        missing_dem_fallback(&world.tiles, ideal, 7),
        Some(grandparent),
        "the walk skips ancestors already known to be missing"
    );
    assert_eq!(
        missing_dem_fallback(&world.tiles, ideal, 12),
        None,
        "nothing below the source minimum zoom"
    );
    assert_eq!(
        missing_dem_fallback(&world.tiles, grandparent, 7),
        None,
        "a tile not known to be missing needs no fallback"
    );
}

use super::request_order;
use crate::tcs::tiles::Tiles;

#[test]
fn foreground_detail_precedes_distant_texture_ancestors() {
    let foreground = WorldTileCoords {
        x: 4354,
        y: 2874,
        z: ZoomLevel::new(13),
    };
    let distant = (0..16).map(|x| WorldTileCoords {
        x,
        y: 0,
        z: ZoomLevel::new(4),
    });
    let wanted = request_order(
        std::iter::once(foreground).chain(distant),
        &Tiles::default(),
        0,
        15,
    );
    assert_eq!(wanted.first().map(|tile| u8::from(tile.z)), Some(5));
    assert_eq!(wanted.get(1), foreground.get_parent().as_ref());
    assert_eq!(
        wanted
            .iter()
            .filter(|tile| **tile == foreground.get_parent().unwrap_or_default())
            .count(),
        1
    );
}

#[test]
fn default_immersive_height_requests_fine_dem_without_head_motion() {
    use cgmath::{Deg, Matrix4, Rad, SquareMatrix, Vector3};

    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        projection::ProjectionType,
        render::{
            camera::EyeFrustum,
            view_state::{ExternalAnchor, ViewState},
            xr::ScenePlacement,
        },
        style::Style,
        window::PhysicalSize,
    };
    let style: Style = serde_json::from_str(
        r#"{"version":8,"sources":{},"layers":[],"projection":{"type":"globe"}}"#,
    )
    .expect("style");
    let position = LatLon::new(47.26, 11.39);
    let zoom = Zoom::new(12.0);
    for pitch in [90.0, 45.0] {
        let mut view = ViewState::new(
            PhysicalSize::new(2048, 2048).expect("size"),
            WorldCoords::from_lat_lon(position, zoom),
            zoom,
            Deg(0.0),
            Rad(1.4),
        );
        let placement = ScenePlacement {
            anchor: ExternalAnchor {
                position,
                altitude_meters: 0.0,
            },
            world_from_scene: Matrix4::identity(),
        };
        let eye = Matrix4::from_translation(Vector3::new(0.0, 0.0, 4000.0))
            * Matrix4::from_angle_x(Deg(pitch));
        let external = placement
            .view_from(eye, EyeFrustum::symmetric(Rad(1.4), 1.0, 0.05, 1e8))
            .expect("eye");
        view.set_external_view(external, &ProjectionType::Globe)
            .expect("view");
        let world = World::default();
        let region = crate::render::projection::view_region_for_projection(
            &style,
            &view,
            &world,
            view.zoom().zoom_level(512.0),
            crate::render::view_state::ViewStatePadding::Tight,
        )
        .expect("covering")
        .expect("visible surface");
        let wanted = request_order(region.iter(), &world.tiles, 0, 15);
        assert!(
            wanted.iter().any(|tile| u8::from(tile.z) >= 11),
            "default pitch {pitch} must request detailed DEMs: {wanted:?}"
        );
    }
}
