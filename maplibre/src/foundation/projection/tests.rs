#![allow(clippy::expect_used, clippy::panic)]

use super::{project_tile_coordinates_to_unit_sphere, ProjectionSpecification, ProjectionType};
use crate::{coords::EXTENT, style::Style};

fn assert_close(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-12, "{left} != {right}");
}

#[test]
fn parses_globe_projection_specification() {
    let projection: ProjectionSpecification =
        serde_json::from_str(r#"{"type":"globe"}"#).expect("projection should deserialize");

    assert_eq!(projection.projection_type, ProjectionType::Globe);
}

#[test]
fn parses_vertical_perspective_projection_specification() {
    let specification: ProjectionSpecification =
        serde_json::from_str(r#"{"type":"vertical-perspective"}"#)
            .expect("vertical-perspective projection should deserialize");

    assert_eq!(
        specification.projection_type,
        ProjectionType::VerticalPerspective
    );
    assert_eq!(specification.projection_type.globe_transition(30.0), 1.0);
}

#[test]
fn projection_expression_interpolates_and_round_trips() {
    let json =
        r#"{"type":["interpolate",["linear"],["zoom"],10,"vertical-perspective",12,"mercator"]}"#;
    let specification: ProjectionSpecification =
        serde_json::from_str(json).expect("projection expression should deserialize");

    assert_eq!(specification.projection_type.globe_transition(9.0), 1.0);
    assert_eq!(specification.projection_type.globe_transition(11.0), 0.5);
    assert_eq!(specification.projection_type.globe_transition(13.0), 0.0);
    assert_eq!(
        serde_json::from_str::<ProjectionSpecification>(
            &serde_json::to_string(&specification).expect("projection should serialize"),
        )
        .expect("serialized projection should deserialize"),
        specification
    );
}

#[test]
fn projection_expression_rejects_invalid_zoom_stops() {
    let result = serde_json::from_str::<ProjectionSpecification>(
        r#"{"type":["interpolate",["linear"],["zoom"],12,"vertical-perspective",10,"mercator"]}"#,
    );

    assert!(result.is_err());
}

#[test]
fn defaults_to_mercator_projection() {
    assert_eq!(ProjectionType::default(), ProjectionType::Mercator);
}

#[test]
fn parses_globe_projection_from_style() {
    let style: Style = serde_json::from_str(
        r#"{"version":8,"sources":{},"layers":[],"projection":{"type":"globe"}}"#,
    )
    .expect("style should deserialize");

    assert_eq!(
        style
            .projection
            .as_ref()
            .map(|value| value.projection_type.clone()),
        Some(ProjectionType::Globe)
    );
}

#[test]
fn style_projection_is_optional() {
    let style: Style = serde_json::from_str(r#"{"version":8,"sources":{},"layers":[]}"#)
        .expect("style should deserialize");

    assert_eq!(style.projection, None);
}

#[test]
fn projects_prime_meridian_and_equator_to_positive_z() {
    let point = project_tile_coordinates_to_unit_sphere(0, 0, 0, EXTENT / 2.0, EXTENT / 2.0);

    assert_close(point.x, 0.0);
    assert_close(point.y, 0.0);
    assert_close(point.z, 1.0);
}

#[test]
fn projected_points_stay_on_unit_sphere() {
    let point = project_tile_coordinates_to_unit_sphere(2, 1, 3, 1234.0, 3456.0);

    assert_close(
        point.x * point.x + point.y * point.y + point.z * point.z,
        1.0,
    );
}

#[test]
fn adjacent_tiles_share_the_same_projected_edge() {
    let left_edge = project_tile_coordinates_to_unit_sphere(0, 0, 1, EXTENT, EXTENT / 2.0);
    let right_edge = project_tile_coordinates_to_unit_sphere(1, 0, 1, 0.0, EXTENT / 2.0);

    assert_close(left_edge.x, right_edge.x);
    assert_close(left_edge.y, right_edge.y);
    assert_close(left_edge.z, right_edge.z);
}

#[test]
fn web_mercator_bounds_map_to_opposite_latitudes() {
    let north = project_tile_coordinates_to_unit_sphere(0, 0, 0, EXTENT / 2.0, 0.0);
    let south = project_tile_coordinates_to_unit_sphere(0, 0, 0, EXTENT / 2.0, EXTENT);

    assert_close(north.y, -south.y);
    assert!(north.y > 0.99);
}

#[test]
fn exponential_projection_interpolation_uses_actual_zoom_span() {
    let spec: ProjectionSpecification = serde_json::from_str(
        r#"{"type":["interpolate",["exponential",2],["zoom"],3,"vertical-perspective",7,"mercator"]}"#,
    )
    .expect("exponential projection should deserialize");

    // At z=5 over [3, 7] with base 2: factor = (2^(5-3) - 1) / (2^(7-3) - 1) = 3 / 15 = 0.2.
    // From vertical-perspective (1.0) to mercator (0.0) => 1.0 - 0.2 = 0.8.
    let at_5 = f64::from(spec.projection_type.globe_transition(5.0));
    assert!(
        (at_5 - 0.8).abs() < 1e-6,
        "expected globe transition 0.8 at z=5, got {at_5}"
    );
    assert_eq!(spec.projection_type.globe_transition(3.0), 1.0);
    assert_eq!(spec.projection_type.globe_transition(7.0), 0.0);
}

#[test]
fn projection_type_resolves_global_state_directly_and_in_expressions() {
    let mut direct: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "state": { "proj": { "default": "mercator" } },
        "projection": { "type": ["global-state", "proj"] },
        "sources": {},
        "layers": []
    }))
    .expect("direct global-state projection parses");
    assert_eq!(
        direct.projection.as_ref().map(|p| &p.projection_type),
        Some(&ProjectionType::Mercator)
    );
    direct.set_global_state("proj", serde_json::json!("globe"));
    assert_eq!(
        direct.projection.as_ref().map(|p| &p.projection_type),
        Some(&ProjectionType::Globe)
    );
    direct.set_global_state("proj", serde_json::Value::Null);
    assert_eq!(
        direct.projection.as_ref().map(|p| &p.projection_type),
        Some(&ProjectionType::Mercator)
    );

    // Inside `interpolate`:
    let mut interp: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "state": { "lowProj": { "default": "vertical-perspective" } },
        "projection": {
            "type": ["interpolate", ["linear"], ["zoom"], 3, ["global-state", "lowProj"], 5, "mercator"]
        },
        "sources": {},
        "layers": []
    }))
    .expect("interpolate global-state projection parses");
    assert!(
        (interp
            .projection
            .as_ref()
            .expect("projection")
            .projection_type
            .globe_transition(4.0)
            - 0.5)
            .abs()
            < 1e-6
    );
    interp.set_global_state("lowProj", serde_json::json!("mercator"));
    assert_eq!(
        interp
            .projection
            .as_ref()
            .expect("projection")
            .projection_type
            .globe_transition(4.0),
        0.0
    );

    // Inside `step`:
    let mut stepped: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "state": { "lowProj": { "default": "vertical-perspective" } },
        "projection": {
            "type": ["step", ["zoom"], ["global-state", "lowProj"], 5, "mercator"]
        },
        "sources": {},
        "layers": []
    }))
    .expect("step global-state projection parses");
    assert_eq!(
        stepped
            .projection
            .as_ref()
            .expect("projection")
            .projection_type
            .globe_transition(4.0),
        1.0
    );
    stepped.set_global_state("lowProj", serde_json::json!("mercator"));
    assert_eq!(
        stepped
            .projection
            .as_ref()
            .expect("projection")
            .projection_type
            .globe_transition(4.0),
        0.0
    );

    // Inside `case`:
    let mut cased: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "state": { "useGlobe": { "default": false } },
        "projection": {
            "type": ["case", ["==", ["global-state", "useGlobe"], true], "vertical-perspective", "mercator"]
        },
        "sources": {},
        "layers": []
    }))
    .expect("case global-state projection parses");
    assert_eq!(
        cased.projection.as_ref().map(|p| &p.projection_type),
        Some(&ProjectionType::Mercator)
    );
    cased.set_global_state("useGlobe", serde_json::json!(true));
    assert_eq!(
        cased.projection.as_ref().map(|p| &p.projection_type),
        Some(&ProjectionType::VerticalPerspective)
    );
}
