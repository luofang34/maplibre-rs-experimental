#![allow(clippy::expect_used)]

use std::error::Error;

use super::*;

#[test]
fn graph_initialization_error_exposes_its_original_cause() {
    let error = MapError::RenderGraphInit(RenderGraphError::WrongNodeType);
    assert!(matches!(
        error
            .source()
            .expect("graph cause")
            .downcast_ref::<RenderGraphError>(),
        Some(RenderGraphError::WrongNodeType)
    ));
}

#[test]
fn renderer_initialization_error_preserves_the_nested_graph_cause() {
    let error = MapError::DeviceInit(RenderError::Graph(RenderGraphError::WrongNodeType));
    let renderer = error
        .source()
        .expect("renderer cause")
        .downcast_ref::<RenderError>()
        .expect("original renderer type");
    assert!(matches!(
        renderer
            .source()
            .expect("graph cause")
            .downcast_ref::<RenderGraphError>(),
        Some(RenderGraphError::WrongNodeType)
    ));
}

#[test]
fn a_jump_before_the_renderer_is_ready_moves_the_view_the_map_starts_with() {
    use crate::{coords::LatLon, window::PhysicalSize};
    let style = Style::default();
    let size = PhysicalSize::new(800, 600).expect("size");
    let view = starting_view(
        &style,
        None,
        size,
        DEFAULT_MAX_PITCH,
        Some(CameraOptions {
            center: Some(LatLon::new(90.0, 11.6)),
            zoom: Some(12.0),
            bearing: Some(cgmath::Deg(45.0)),
            ..Default::default()
        }),
    );
    let center = view.center_lat_lon();
    assert!(
        center.latitude.is_finite() && center.latitude < 86.0,
        "the latitude is kept on the map as a jump on a ready map keeps it: {center}"
    );
    assert!((center.longitude - 11.6).abs() < 1e-9, "{center}");
    assert!((view.zoom().value() - 12.0).abs() < 1e-12);
    assert!((cgmath::Deg::from(view.camera().get_bearing()).0 - 45.0).abs() < 1e-9);
    let written = Style::default();
    assert_eq!(
        (style.center, style.zoom, style.bearing),
        (written.center, written.zoom, written.bearing),
        "the style's own camera is left as written"
    );

    // A map reset keeps its view, and a jump made before it comes back moves that view.
    let retained = starting_view(&style, None, size, DEFAULT_MAX_PITCH, None);
    let moved = starting_view(
        &style,
        Some(&retained),
        size,
        DEFAULT_MAX_PITCH,
        Some(CameraOptions {
            zoom: Some(5.0),
            ..Default::default()
        }),
    );
    assert!((moved.zoom().value() - 5.0).abs() < 1e-12);
    assert_eq!(moved.center_lat_lon(), retained.center_lat_lon());

    let both = merged(
        Some(CameraOptions {
            zoom: Some(3.0),
            bearing: Some(cgmath::Deg(10.0)),
            ..Default::default()
        }),
        CameraOptions {
            zoom: Some(4.0),
            ..Default::default()
        },
    );
    assert_eq!(
        both.zoom,
        Some(4.0),
        "a later jump's field replaces an earlier one's"
    );
    assert_eq!(
        both.bearing,
        Some(cgmath::Deg(10.0)),
        "what it leaves out is kept"
    );
}
