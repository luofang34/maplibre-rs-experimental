//! A free-globe camera on a running map: the frame ends it when the style's projection
//! stops allowing it, and a drag the flat camera cannot follow still counts as a move.

use cgmath::Point2;

use crate::{
    coords::LatLon,
    projection::{ProjectionSpecification, ProjectionType},
    render::view_state::{NavigationLimit, NavigationMode},
};

async fn free_map(center: LatLon) -> super::super::HeadlessMap {
    let style: crate::style::Style = serde_json::from_str(&format!(
        r#"{{"version":8,"center":[{},{}],"zoom":5,"pitch":30,
            "projection":{{"type":"vertical-perspective"}},"sources":{{}},"layers":[]}}"#,
        center.longitude, center.latitude
    ))
    .expect("style");
    let (kernel, renderer) = crate::headless::create_headless_renderer(64, 64, None)
        .await
        .expect("renderer");
    let mut map = super::super::HeadlessMap::new(
        style,
        renderer,
        kernel,
        vec![Box::new(crate::render::RenderPlugin)],
    )
    .expect("map");
    map.set_navigation_mode(NavigationMode::FreeGlobe)
        .expect("free navigation");
    map
}

#[tokio::test]
async fn a_frame_ends_free_navigation_when_the_style_projection_changes() {
    let mut map = free_map(LatLon::new(40.0, 10.0)).await;
    map.run_frame().expect("frame");
    assert_eq!(
        map.view_state().navigation_mode(),
        NavigationMode::FreeGlobe
    );
    map.map_context.style.projection = Some(ProjectionSpecification {
        projection_type: ProjectionType::Globe,
    });
    map.run_frame().expect("frame");
    assert_eq!(
        map.view_state().navigation_mode(),
        NavigationMode::NorthLocked
    );
    assert_eq!(
        map.view_state().navigation_limit(),
        Some(NavigationLimit::ProjectionChanged)
    );
}

#[tokio::test]
async fn a_drag_over_a_cap_counts_as_a_move_though_the_flat_camera_stays() {
    let mut map = free_map(LatLon::new(84.0, 10.0)).await;
    // Drag north until the camera looks at the cap.
    let center = Point2::new(32.0, 32.0);
    while map
        .view_state()
        .pose_view()
        .expect("free camera")
        .center
        .latitude
        < 86.0
    {
        assert!(map
            .map_context
            .view_state
            .drag_free_globe(center - cgmath::Vector2::new(0.0, 8.0), center));
    }
    map.run_frame().expect("frame");
    let state = map.view_state();
    let flat = (
        state.zoom(),
        state.camera().position(),
        state.camera().get_bearing(),
        state.camera().get_pitch(),
        state.camera().get_roll(),
    );
    assert!(map
        .map_context
        .view_state
        .drag_free_globe(center - cgmath::Vector2::new(0.0, 2.0), center));
    // Over the cap the flat camera stays at the last row of tiles, but for rounding; put it
    // back exactly, so only the pose tells the move.
    let state = &mut map.map_context.view_state;
    assert!((state.camera().position() - flat.1).x.abs() < 1e-6);
    state.update_zoom(flat.0);
    state.camera_mut().move_to(flat.1);
    state.camera_mut().set_bearing(flat.2);
    state.camera_mut().set_pitch(flat.3);
    state.camera_mut().set_roll(flat.4);
    assert!(
        crate::render::frame_signals::camera_moved(&map.map_context.world, map.view_state()),
        "the free camera moved"
    );
}
