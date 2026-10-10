use super::*;
use crate::{
    coords::{LatLon, WorldCoords, Zoom, ZoomLevel},
    projection::{ProjectionSpecification, ProjectionType},
    render::{projection::projection_data_for_view, view_state::NavigationMode},
    style::{layer::SymbolPaint, Style},
    window::PhysicalSize,
};

#[test]
fn free_camera_symbols_keep_their_pixel_size_at_nearby_visible_anchors() {
    let style = Style {
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::VerticalPerspective,
        }),
        ..Default::default()
    };
    let mut view = ViewState::new(
        PhysicalSize::new(800, 600).expect("viewport"),
        WorldCoords::from_lat_lon(LatLon::new(0.0, 0.0), Zoom::new(3.0)),
        Zoom::new(3.0),
        cgmath::Deg(0.0),
        cgmath::Deg(36.869_897_645_844_02),
    );
    view.set_max_pitch(cgmath::Deg(85.0));
    view.camera_mut().set_pitch(cgmath::Deg(75.0));
    view.set_globe_orbits_center(true);
    view.set_navigation_mode(
        NavigationMode::FreeGlobe,
        &ProjectionType::VerticalPerspective,
    )
    .expect("free camera");
    let projection = projection_data_for_view(&style, &view).expect("projection");
    assert_eq!(projection.globe_circle[2], 1.0);
    assert_eq!(projection.external_view, 0.0);
    let mut uniforms = SymbolUniforms::new(&SymbolPaint::default(), 3.0, [1, 1]);
    uniforms.icon_layout = [0.0; 4];
    uniforms.placement[1] = 0.0;
    let part = SymbolBounds {
        bounds: [-12.0, -12.0, 12.0, 12.0],
        height: 0.0,
        angle: 0.0,
        text: false,
    };
    let coords = WorldTileCoords {
        x: 0,
        y: 0,
        z: ZoomLevel::new(0),
    };
    let mut visible = 0;
    let mut greatest_depth_ratio = 0.0_f64;
    for y in (1536..3072).step_by(16) {
        let anchor = [2048.0, f64::from(y)];
        let placement = Placement {
            coords,
            anchor,
            elevation: 0.0,
            view: &view,
            projection: &projection,
            uniforms: &uniforms,
        };
        if let Some(bounds) = placement.bounds(&part) {
            if bounds[0] < 0.0 || bounds[2] > 800.0 || bounds[1] < 0.0 || bounds[3] > 600.0 {
                continue;
            }
            let clip = project(coords, anchor, 0.0, &view, &projection).expect("visible anchor");
            greatest_depth_ratio =
                greatest_depth_ratio.max(f64::from(projection.center_clip_w) / clip.w);
            assert!((bounds[2] - bounds[0] - 24.0).abs() < 1e-6);
            assert!((bounds[3] - bounds[1] - 24.0).abs() < 1e-6);
            visible += 1;
        }
    }
    assert!(visible > 2);
    assert!(
        greatest_depth_ratio > 1.2,
        "nearby anchors reproduce depth amplification: {greatest_depth_ratio}"
    );
}

#[test]
fn free_symbol_scale_survives_precise_globe_projection_without_changing_world_up() {
    let style = Style {
        projection: Some(ProjectionSpecification {
            projection_type: ProjectionType::VerticalPerspective,
        }),
        ..Default::default()
    };
    for zoom in [3.0, 14.0] {
        let mut view = ViewState::new(
            PhysicalSize::new(800, 600).expect("viewport"),
            WorldCoords::from_lat_lon(LatLon::new(0.0, 0.0), Zoom::new(zoom)),
            Zoom::new(zoom),
            cgmath::Deg(0.0),
            cgmath::Deg(36.869_897_645_844_02),
        );
        let normal = projection_data_for_view(&style, &view).expect("normal projection");
        assert_eq!(normal.globe_circle[2], 0.0);
        view.set_navigation_mode(
            NavigationMode::FreeGlobe,
            &ProjectionType::VerticalPerspective,
        )
        .expect("free camera");
        let free = projection_data_for_view(&style, &view).expect("free projection");
        assert_eq!(free.globe_circle[2], 1.0);
        assert_eq!(free.external_view, 0.0);
        view.set_navigation_mode(
            NavigationMode::Constrained,
            &ProjectionType::VerticalPerspective,
        )
        .expect("constrained camera");
        assert_eq!(
            projection_data_for_view(&style, &view)
                .expect("restored projection")
                .globe_circle[2],
            0.0
        );
    }
}
