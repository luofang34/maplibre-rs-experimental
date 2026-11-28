use geozero::{FeatureProcessor, GeomProcessor, PropertyProcessor};

use super::{IndexDataType, ZeroTessellator};
use crate::style::layer::StyleProperty;

#[test]
fn globe_line_tessellation_inserts_grid_crossings() {
    let mut mercator = ZeroTessellator::<IndexDataType>::default();
    tessellate_reference_line(&mut mercator);
    let mut globe =
        ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(4, false, false, false);
    tessellate_reference_line(&mut globe);

    assert!(globe.buffer.vertices.len() > mercator.buffer.vertices.len());
    assert!(globe.buffer.indices.len() > mercator.buffer.indices.len());
}

#[test]
fn globe_fill_tessellation_cuts_triangle_interior() {
    let mut mercator = ZeroTessellator::<IndexDataType>::default();
    tessellate_reference_triangle(&mut mercator);
    let mut globe =
        ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(2, false, false, false);
    tessellate_reference_triangle(&mut globe);

    assert!(globe.buffer.indices.len() > mercator.buffer.indices.len());
    assert!(globe
        .buffer
        .vertices
        .iter()
        .any(|vertex| vertex.position == [2048.0, 0.0]));
}

#[test]
fn features_with_greater_sort_keys_are_drawn_later() {
    let mut tessellator = ZeroTessellator::<IndexDataType> {
        sort_key: super::SortKeys::by(Some(StyleProperty::parse(&serde_json::json!([
            "get", "rank"
        ])))),
        fallback_color: [0.0; 4],
        ..Default::default()
    };
    for (rank, x) in [(2.0, 100.0), (0.0, 200.0), (1.0, 300.0)] {
        tessellator
            .property(0, "rank", &geozero::ColumnValue::Double(rank))
            .expect("rank is accepted");
        tessellator
            .polygon_begin(true, 1, 0)
            .expect("polygon begins");
        tessellator
            .linestring_begin(false, 4, 0)
            .expect("ring begins");
        for (dx, dy) in [(0.0, 0.0), (50.0, 0.0), (50.0, 50.0), (0.0, 0.0)] {
            tessellator.xy(x + dx, dy, 0).expect("coordinate is valid");
        }
        tessellator.linestring_end(false, 0).expect("ring ends");
        tessellator.polygon_end(true, 0).expect("polygon ends");
        tessellator.feature_end(0).expect("feature ends");
    }
    tessellator.apply_sort_keys();

    let firsts: Vec<f32> = tessellator
        .feature_indices
        .iter()
        .scan(0_usize, |start, count| {
            let first = *start;
            *start += *count as usize;
            Some(tessellator.buffer.vertices[first].position[0])
        })
        .collect();
    assert_eq!(firsts, [200.0, 300.0, 100.0]);
    let vertices = tessellator.buffer.vertices.len() as u32;
    assert!(tessellator
        .buffer
        .indices
        .iter()
        .all(|index| *index < vertices));
}

/// The y of every vertex the globe shader draws at a pole, by vertex index.
fn pole_vertices(tessellator: &ZeroTessellator<IndexDataType>) -> Vec<(usize, f32)> {
    tessellator
        .buffer
        .vertices
        .iter()
        .enumerate()
        .filter(|(_, vertex)| {
            vertex.position[1] == i16::MIN as f32 || vertex.position[1] == i16::MAX as f32
        })
        .map(|(index, vertex)| (index, vertex.position[1]))
        .collect()
}

fn square_ring(tessellator: &mut ZeroTessellator<IndexDataType>, min: f64, max: f64) {
    tessellator
        .polygon_begin(false, 1, 0)
        .expect("polygon begins");
    tessellator
        .linestring_begin(false, 5, 0)
        .expect("ring begins");
    for (index, (x, y)) in [(min, min), (max, min), (max, max), (min, max), (min, min)]
        .into_iter()
        .enumerate()
    {
        tessellator.xy(x, y, index).expect("coordinate is valid");
    }
    tessellator.linestring_end(false, 0).expect("ring ends");
    tessellator.polygon_end(false, 0).expect("polygon ends");
}

/// Tessellates a polygon covering the whole z0 tile, from pole edge to pole edge, then a lake
/// inside it, each as a feature of its own or as the polygons of one multipolygon.
fn tessellate_ocean_then_lake(
    tessellator: &mut ZeroTessellator<IndexDataType>,
    multipolygon: bool,
) {
    if multipolygon {
        tessellator
            .multipolygon_begin(2, 0)
            .expect("multipolygon begins");
    }
    for (min, max) in [(0.0, 4096.0), (1000.0, 1500.0)] {
        if !multipolygon {
            tessellator
                .multipolygon_begin(1, 0)
                .expect("multipolygon begins");
        }
        square_ring(tessellator, min, max);
        if !multipolygon {
            tessellator.multipolygon_end(0).expect("multipolygon ends");
            tessellator.feature_end(0).expect("feature ends");
        }
    }
    if multipolygon {
        tessellator.multipolygon_end(0).expect("multipolygon ends");
        tessellator.feature_end(0).expect("feature ends");
    }
}

fn assert_poles_survive_later_polygons(
    mut tessellator: ZeroTessellator<IndexDataType>,
    multipolygon: bool,
) {
    let mut ocean_only =
        ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(2, true, true, true);
    ocean_only.extrusion = tessellator.extrusion.clone();
    ocean_only
        .multipolygon_begin(1, 0)
        .expect("multipolygon begins");
    square_ring(&mut ocean_only, 0.0, 4096.0);
    ocean_only.multipolygon_end(0).expect("multipolygon ends");
    let poles = pole_vertices(&ocean_only);
    assert!(
        poles.iter().any(|(_, y)| *y == i16::MIN as f32),
        "north pole"
    );
    assert!(
        poles.iter().any(|(_, y)| *y == i16::MAX as f32),
        "south pole"
    );

    tessellate_ocean_then_lake(&mut tessellator, multipolygon);

    assert_eq!(pole_vertices(&tessellator), poles);
}

#[test]
fn later_fill_features_leave_both_poles_exact() {
    for multipolygon in [false, true] {
        let tessellator =
            ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(2, true, true, true);
        assert_poles_survive_later_polygons(tessellator, multipolygon);
    }
}

#[test]
fn later_extrusion_polygons_leave_both_poles_exact() {
    for multipolygon in [false, true] {
        let tessellator = ZeroTessellator::<IndexDataType>::default()
            .with_globe_subdivision(2, true, true, true)
            .with_extrusion(super::ExtrusionOptions {
                height: None,
                base: None,
                base_datum: None,
                height_datum: None,
                part: None,
                translucent: false,
                clip_buffer: None,
                rounded_corner_distance: None,
                has_pattern: false,
                overscale: 0,
            });
        assert_poles_survive_later_polygons(tessellator, multipolygon);
    }
}

#[test]
fn extrusion_skips_linestring_and_multilinestring_geometries() {
    let mut tessellator =
        ZeroTessellator::<IndexDataType>::default().with_extrusion(super::ExtrusionOptions {
            height: Some(crate::style::layer::StyleProperty::Constant(100.0)),
            base: None,
            base_datum: None,
            height_datum: None,
            part: None,
            translucent: false,
            clip_buffer: None,
            rounded_corner_distance: None,
            has_pattern: false,
            overscale: 0,
        });

    // Acute-angle (< 83.6 deg) LineString that could emit degenerate extrusion spikes.
    tessellator.feature_begin(0).expect("feature begins");
    tessellator
        .linestring_begin(true, 3, 0)
        .expect("line begins");
    tessellator.xy(1000.0, 1000.0, 0).expect("xy");
    tessellator.xy(2000.0, 2000.0, 1).expect("xy");
    tessellator.xy(1100.0, 1000.0, 2).expect("xy");
    tessellator.linestring_end(true, 0).expect("line ends");
    tessellator.feature_end(0).expect("feature ends");

    // Acute-angle MultiLineString.
    tessellator.feature_begin(1).expect("feature begins");
    tessellator
        .multilinestring_begin(1, 0)
        .expect("multiline begins");
    tessellator
        .linestring_begin(false, 3, 0)
        .expect("line begins");
    tessellator.xy(500.0, 500.0, 0).expect("xy");
    tessellator.xy(1500.0, 1500.0, 1).expect("xy");
    tessellator.xy(600.0, 500.0, 2).expect("xy");
    tessellator.linestring_end(false, 0).expect("line ends");
    tessellator.multilinestring_end(0).expect("multiline ends");
    tessellator.feature_end(1).expect("feature ends");

    assert!(
        tessellator.buffer.vertices.is_empty(),
        "fill-extrusion must emit zero vertices for LineString/MultiLineString"
    );
    assert!(
        tessellator.buffer.indices.is_empty(),
        "fill-extrusion must emit zero indices for LineString/MultiLineString"
    );

    // Polygons on the same extrusion tessellator must still tessellate normally.
    tessellator.feature_begin(2).expect("feature begins");
    tessellate_reference_triangle(&mut tessellator);
    tessellator.feature_end(2).expect("feature ends");
    assert!(
        !tessellator.buffer.vertices.is_empty() && !tessellator.buffer.indices.is_empty(),
        "fill-extrusion must still tessellate Polygon geometries"
    );
}

fn tessellate_reference_line(tessellator: &mut ZeroTessellator<IndexDataType>) {
    tessellator
        .linestring_begin(true, 2, 0)
        .expect("line should begin");
    tessellator
        .xy(0.0, 0.0, 0)
        .expect("line start should be valid");
    tessellator
        .xy(4096.0, 0.0, 1)
        .expect("line end should be valid");
    tessellator
        .linestring_end(true, 0)
        .expect("line should tessellate");
}

fn tessellate_reference_triangle(tessellator: &mut ZeroTessellator<IndexDataType>) {
    tessellator
        .polygon_begin(true, 1, 0)
        .expect("polygon should begin");
    tessellator
        .linestring_begin(false, 4, 0)
        .expect("ring should begin");
    for (index, point) in [[0.0, 0.0], [4096.0, 0.0], [0.0, 4096.0], [0.0, 0.0]]
        .into_iter()
        .enumerate()
    {
        tessellator
            .xy(point[0], point[1], index)
            .expect("ring coordinate should be valid");
    }
    tessellator
        .linestring_end(false, 0)
        .expect("ring should end");
    tessellator
        .polygon_end(true, 0)
        .expect("polygon should tessellate");
}

mod circles;
