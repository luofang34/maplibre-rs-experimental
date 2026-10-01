use geozero::{FeatureProcessor, GeomProcessor, PropertyProcessor};

use crate::style::layer::StyleProperty;

use super::{IndexDataType, ZeroTessellator};

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

mod circles {
    #![allow(clippy::expect_used, clippy::panic)]

    use geozero::{ColumnValue, FeatureProcessor, GeomProcessor, PropertyProcessor};

    use crate::{
        style::layer::StyleProperty,
        vector::tessellation::{
            CircleOptions, IndexDataType, ZeroTessellator, CIRCLE_QUAD_INDICES,
        },
    };

    fn circle_tessellator(
        radius: StyleProperty<f32>,
        stroke_width: f32,
    ) -> ZeroTessellator<IndexDataType> {
        ZeroTessellator::<IndexDataType>::default()
            .with_circles(CircleOptions {
                radius,
                radius_default: 5.0,
                stroke_width: StyleProperty::Constant(stroke_width),
                stroke_width_default: 0.0,
                blur: None,
                stroke_opacity: None,
                zoom: 3.0,
                grid: false,
            })
            .with_feature_opacity(
                Some(StyleProperty::parse(&serde_json::json!(["get", "alpha"]))),
                3.0,
            )
    }

    #[test]
    fn feature_opacity_is_folded_into_the_colour_alpha() {
        let mut tessellator = circle_tessellator(StyleProperty::Constant(3.0), 0.0);
        tessellator
            .property(0, "alpha", &ColumnValue::Double(0.25))
            .expect("property");
        point(&mut tessellator, 5.0, 5.0);
        tessellator.feature_end(0).expect("feature ends");
        point(&mut tessellator, 6.0, 6.0);
        tessellator.feature_end(1).expect("feature ends");

        assert_eq!(tessellator.feature_colors[0][3], 0.25);
        assert_eq!(
            tessellator.feature_colors[1][3], 1.0,
            "a missing opacity is opaque"
        );
    }

    fn point(tessellator: &mut ZeroTessellator<IndexDataType>, x: f64, y: f64) {
        tessellator.point_begin(0).expect("point begins");
        tessellator.xy(x, y, 0).expect("point coordinate");
        tessellator.point_end(0).expect("point ends");
    }

    #[test]
    fn a_radius_that_varies_by_feature_and_zoom_also_carries_the_next_zoom() {
        let radius = StyleProperty::parse(&serde_json::json!({
            "property": "size", "type": "exponential",
            "stops": [[{"zoom": 3, "value": 0}, 2], [{"zoom": 4, "value": 0}, 10]]
        }));
        let mut tessellator = circle_tessellator(radius, 0.0);
        tessellator
            .property(0, "size", &ColumnValue::Double(0.0))
            .expect("property");
        point(&mut tessellator, 5.0, 5.0);
        tessellator.feature_end(0).expect("feature ends");

        let vertex = &tessellator.buffer.vertices[0];
        assert_eq!(vertex.normal[0], 2.0);
        assert_eq!(vertex.edge_distance, 10.0);
    }

    #[test]
    fn a_circle_on_the_globe_is_a_grid_of_sixteen_vertices() {
        let mut tessellator =
            ZeroTessellator::<IndexDataType>::default().with_circles(CircleOptions {
                radius: StyleProperty::Constant(4.0),
                radius_default: 5.0,
                stroke_width: StyleProperty::Constant(0.0),
                stroke_width_default: 0.0,
                blur: None,
                stroke_opacity: None,
                zoom: 3.0,
                grid: true,
            });
        point(&mut tessellator, 100.0, 200.0);
        tessellator.feature_end(0).expect("feature ends");

        assert_eq!(tessellator.buffer.vertices.len(), 16);
        assert_eq!(tessellator.buffer.indices.len(), 3 * 3 * 6);
        let codes: Vec<f32> = tessellator
            .buffer
            .vertices
            .iter()
            .map(|vertex| vertex.corner_code)
            .collect();
        assert_eq!(codes[0], 1.0, "the first corner is the grid's origin");
        assert_eq!(
            codes[15],
            1.0 + 7.0 * 8.0 + 7.0,
            "the last is its far corner"
        );
        assert_eq!(tessellator.feature_indices, vec![16]);
    }

    #[test]
    fn a_point_becomes_a_quad_carrying_radius_and_stroke() {
        let mut tessellator = circle_tessellator(StyleProperty::Constant(4.0), 1.5);
        point(&mut tessellator, 100.0, 200.0);
        tessellator.feature_end(0).expect("feature ends");

        assert_eq!(tessellator.buffer.vertices.len(), 4);
        for vertex in &tessellator.buffer.vertices {
            assert_eq!(vertex.position, [100.0, 200.0]);
            assert_eq!(vertex.normal, [4.0, 1.5]);
        }
        assert_eq!(tessellator.buffer.indices, CIRCLE_QUAD_INDICES.to_vec());
        assert_eq!(tessellator.feature_indices, vec![4]);
    }

    #[test]
    fn points_outside_the_tile_are_left_to_the_neighbour() {
        let mut tessellator = circle_tessellator(StyleProperty::Constant(4.0), 0.0);
        point(&mut tessellator, -1.0, 10.0);
        point(&mut tessellator, 10.0, 4096.0);
        point(&mut tessellator, 4095.0, 0.0);

        assert_eq!(
            tessellator.buffer.vertices.len(),
            4,
            "only the in-tile point is drawn"
        );
    }

    #[test]
    fn the_radius_follows_feature_properties() {
        let mut tessellator = circle_tessellator(
            StyleProperty::parse(&serde_json::json!(["get", "size"])),
            0.0,
        );
        tessellator
            .property(0, "size", &ColumnValue::Double(9.0))
            .expect("property");
        point(&mut tessellator, 5.0, 5.0);
        tessellator.feature_end(0).expect("feature ends");
        tessellator
            .property(0, "size", &ColumnValue::Double(2.0))
            .expect("property");
        point(&mut tessellator, 6.0, 6.0);
        tessellator.feature_end(1).expect("feature ends");

        let radii: Vec<f32> = tessellator
            .buffer
            .vertices
            .iter()
            .step_by(4)
            .map(|vertex| vertex.normal[0])
            .collect();
        assert_eq!(radii, vec![9.0, 2.0]);
        assert_eq!(tessellator.feature_indices, vec![4, 4]);
    }

    #[test]
    fn every_vertex_of_a_line_gets_a_circle_and_no_stroke_geometry() {
        let mut tessellator = circle_tessellator(StyleProperty::Constant(3.0), 0.0);
        tessellator
            .linestring_begin(true, 3, 0)
            .expect("line begins");
        tessellator.xy(10.0, 10.0, 0).expect("coordinate");
        tessellator.xy(20.0, 10.0, 1).expect("coordinate");
        tessellator.xy(30.0, 10.0, 2).expect("coordinate");
        tessellator.linestring_end(true, 0).expect("line ends");

        assert_eq!(tessellator.buffer.vertices.len(), 12);
        assert_eq!(tessellator.buffer.indices.len(), 18);
    }

    #[test]
    fn the_base_index_advances_by_four_per_quad() {
        let mut tessellator = circle_tessellator(StyleProperty::Constant(3.0), 0.0);
        point(&mut tessellator, 1.0, 1.0);
        point(&mut tessellator, 2.0, 2.0);

        assert_eq!(tessellator.buffer.indices[6..], [4, 5, 6, 4, 6, 7]);
    }

    fn heatmap_tessellator(paint: serde_json::Value) -> ZeroTessellator<IndexDataType> {
        let paint: crate::style::heatmap::HeatmapPaint =
            serde_json::from_value(paint).expect("heatmap paint");
        ZeroTessellator::<IndexDataType>::default()
            .with_circles(CircleOptions::for_heatmap(&paint, 3.0))
    }

    #[test]
    fn a_heatmap_point_is_a_unit_quad_carrying_its_weight() {
        let mut tessellator = heatmap_tessellator(serde_json::json!({
            "heatmap-weight": ["get", "mag"]
        }));
        tessellator
            .property(0, "mag", &ColumnValue::Double(0.25))
            .expect("property");
        point(&mut tessellator, 100.0, 200.0);
        tessellator.feature_end(0).expect("feature ends");
        point(&mut tessellator, 300.0, 400.0);
        tessellator.feature_end(1).expect("feature ends");

        assert_eq!(tessellator.buffer.vertices.len(), 8);
        assert_eq!(tessellator.buffer.vertices[0].normal, [1.0, 0.25]);
        assert_eq!(
            tessellator.buffer.vertices[4].normal,
            [1.0, 1.0],
            "a feature without the property counts once"
        );
    }

    #[test]
    fn a_heatmap_without_a_weight_counts_every_point_once() {
        let mut tessellator = heatmap_tessellator(serde_json::json!({}));
        point(&mut tessellator, 10.0, 10.0);
        tessellator.feature_end(0).expect("feature ends");
        assert_eq!(tessellator.buffer.vertices[0].normal, [1.0, 1.0]);
    }

    #[test]
    fn a_per_feature_heatmap_radius_is_the_quads_own_size() {
        let mut tessellator = heatmap_tessellator(serde_json::json!({
            "heatmap-radius": ["get", "size"]
        }));
        tessellator
            .property(0, "size", &ColumnValue::Double(12.0))
            .expect("property");
        point(&mut tessellator, 100.0, 200.0);
        tessellator.feature_end(0).expect("feature ends");
        point(&mut tessellator, 300.0, 400.0);
        tessellator.feature_end(1).expect("feature ends");
        assert_eq!(tessellator.buffer.vertices[0].normal[0], 12.0);
        assert_eq!(
            tessellator.buffer.vertices[4].normal[0], 30.0,
            "a feature without a radius takes the specification default"
        );
    }
}
