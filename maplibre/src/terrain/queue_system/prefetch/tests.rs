use super::*;
use crate::coords::ZoomLevel;
use crate::{
    coords::{LatLon, WorldCoords, Zoom},
    projection::ProjectionType,
    render::{
        camera::EyeFrustum,
        view_state::{ExternalAnchor, ExternalView},
    },
    window::PhysicalSize,
};
use cgmath::{Matrix4, SquareMatrix, Vector3};
#[test]
fn speculative_work_excludes_visible_and_respects_count_and_bytes() {
    let candidates: Vec<_> = (0..20)
        .map(|x| WorldTileCoords::from((x, 0, ZoomLevel::new(8))))
        .collect();
    let selected = select(candidates.clone(), &candidates[..2], 8, |_| 0);
    assert_eq!(selected, candidates[2..6]);
    let selected = select(candidates.clone(), &[], 8, |_| 20 << 20);
    assert_eq!(selected, candidates[..1]);
    assert!(select(candidates, &[], 0, |_| 0).is_empty());
}

#[test]
#[allow(clippy::expect_used, clippy::panic)]
fn moving_terrain_prepares_offscreen_tiles_and_clears_them_under_pressure() {
    let (style, view, external) = immersive_view();
    let mut world = World::default();
    let visible = view_region_for_projection(
        &style,
        &view,
        &world,
        view.zoom().zoom_level(DEFAULT_TILE_SIZE),
        ViewStatePadding::Tight,
    )
    .expect("covering")
    .expect("visible");
    let visible = super::super::covering::bounded_covering(visible.iter(), 24);
    let mut ahead = view.clone();
    ahead
        .set_external_view(
            ExternalView {
                view: Matrix4::from_translation(Vector3::new(10_000.0, 0.0, 4000.0))
                    .invert()
                    .expect("pose"),
                ..external
            },
            &ProjectionType::Mercator,
        )
        .expect("ahead");
    world.resources.insert(PrefetchView {
        view_state: Some(ahead),
        placement: None,
    });
    prepare(&style, &view, &mut world, &visible, MemoryBudget::default());
    let requests = &world
        .resources
        .get::<DrapePrefetchRequests>()
        .expect("requests")
        .0;
    assert!(!requests.is_empty());
    assert!(requests.len() <= 4);
    assert!(requests.iter().all(|tile| !visible.contains(tile)));
    prepare(
        &style,
        &view,
        &mut world,
        &visible,
        MemoryBudget {
            available_bytes: Some(1),
        },
    );
    assert!(world
        .resources
        .get::<DrapePrefetchRequests>()
        .expect("requests")
        .0
        .is_empty());
}

#[allow(clippy::expect_used, clippy::panic)]
fn immersive_view() -> (Style, ViewState, crate::render::view_state::ExternalView) {
    use cgmath::{Deg, Matrix4, Vector3};
    let style: Style = serde_json::from_str(
        r#"{"version":8,"sources":{},"layers":[],"terrain":{"source":"dem"}}"#,
    )
    .expect("style");
    let mut view = ViewState::new(
        PhysicalSize::new(2048, 2048).expect("size"),
        WorldCoords::from((256.0, 256.0)),
        Zoom::new(10.0),
        Deg(0.0),
        Deg(45.0),
    );
    let external = ExternalView {
        anchor: ExternalAnchor {
            position: LatLon::new(47.26, 11.39),
            altitude_meters: 600.0,
        },
        view: Matrix4::from_translation(Vector3::new(0.0, 0.0, -4000.0)),
        frustum: EyeFrustum::symmetric(Deg(60.0).into(), 1.0, 0.05, 1e8),
    };
    view.set_external_view(external, &ProjectionType::Mercator)
        .expect("view");
    (style, view, external)
}
