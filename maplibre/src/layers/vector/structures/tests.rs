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

#[test]
fn extrusion_and_elevated_line_respect_msl_and_agl_datums_over_sloped_ground() {
    use crate::vector::tessellation::pack_extrusion_code;

    let make_pair = |base: f32, height: f32, base_d: ElevationDatum, height_d: ElevationDatum| {
        let code = pack_extrusion_code(0.0, 0, ElevationDatum::pack_pair(base_d, height_d));
        [100.0_f32, 900.0].map(|x| {
            let mut v = ShaderVertex::new([x, 0.0], [0.0, 0.0]);
            v.distance = base;
            v.elevation = height;
            v.corner_code = code;
            v
        })
    };

    // Pure MSL stays completely independent of terrain height.
    let mut msl = make_pair(300.0, 1200.0, ElevationDatum::Msl, ElevationDatum::Msl);
    elevate_span(&mut msl, StructureKind::Extrusion, 6.0, None, &|p| {
        Some(f64::from(p[0]))
    });
    assert_eq!(msl[0].distance, 300.0);
    assert_eq!(msl[1].elevation, 1200.0);

    // Surface/AGL floor + MSL ceiling clamps where sloped terrain rises above the MSL ceiling.
    let mut mixed = make_pair(0.0, 500.0, ElevationDatum::VertexAgl, ElevationDatum::Msl);
    elevate_span(&mut mixed, StructureKind::Extrusion, 6.0, None, &|p| {
        Some(f64::from(p[0]))
    });
    assert_eq!((mixed[0].distance, mixed[0].elevation), (100.0, 500.0));
    assert_eq!((mixed[1].distance, mixed[1].elevation), (900.0, 900.0));

    // ElevatedLine with VertexAgl follows per-vertex ground, while Msl stays fixed.
    let mut agl_line = [100.0_f32, 400.0].map(|x| {
        let mut v = ShaderVertex::new([x, 0.0], [0.0, 1.0]);
        v.elevation = 200.0;
        v.corner_code = f32::from(ElevationDatum::VertexAgl.code());
        v
    });
    elevate_span(
        &mut agl_line,
        StructureKind::ElevatedLine,
        6.0,
        None,
        &|p| Some(f64::from(p[0])),
    );
    assert_eq!(agl_line[0].elevation, 300.0);
    assert_eq!(agl_line[1].elevation, 600.0);
}

mod metadata;
