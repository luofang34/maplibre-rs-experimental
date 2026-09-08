use super::*;
#[test]
fn bridge_spans_valley_and_tunnel_stays_under_hill() {
    assert_eq!(
        profile(StructureKind::Bridge, 100.0, 120.0, 0.0, 0.5, 6.0),
        110.5
    );
    assert_eq!(
        profile(StructureKind::Tunnel, 100.0, 120.0, 500.0, 0.5, 6.0),
        110.1
    );
    assert_eq!(
        profile(StructureKind::Bridge, 100.0, 120.0, 100.0, 0.0, 6.0),
        100.5
    );
    assert_eq!(
        profile(StructureKind::Tunnel, 100.0, 120.0, 120.0, 1.0, 6.0),
        120.1
    );
}
#[test]
fn inferred_flat_crossings_have_clearance_without_treating_osm_layer_as_height() {
    assert_eq!(profile(StructureKind::Bridge, 0.0, 0.0, 0.0, 0.5, 6.0), 6.5);
    assert_eq!(
        profile(StructureKind::Tunnel, 0.0, 0.0, 0.0, 0.5, 6.0),
        -5.9
    );
}
#[test]
fn geometry_uses_absolute_deck_elevation_when_supplied() {
    let mut vertices: Vec<_> = [0.0, 1.0, 2.0]
        .into_iter()
        .map(|distance| {
            let mut v = ShaderVertex::new([distance, 0.0], [0.0, 1.0]);
            v.distance = distance;
            v
        })
        .collect();
    elevate_span(
        &mut vertices,
        StructureKind::Bridge,
        6.0,
        Some(250.0),
        &|_| Some(0.0),
    );
    assert!(vertices.iter().all(|v| v.elevation == 250.0));
}
