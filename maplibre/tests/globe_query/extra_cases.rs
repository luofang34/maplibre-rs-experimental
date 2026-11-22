use geo_types::{LineString, Polygon};
use maplibre::{
    coords::{EXTENT, TILE_SIZE},
    io::geometry_index::IndexedGeometry,
    query::query_rendered_features,
    render::resolved_view::ResolvedFrameView,
    sdf::query::{QueryGeometry, QueryOptions},
    tcs::world::World,
};

use super::{fill_style, h::*};

// 13: Heatmap radius hit & miss on globe including whole_visible_hemisphere.
#[test]
fn c13_heatmap_radius_on_globe() {
    let style = globe_style(serde_json::json!([
        {"id": "hm", "type": "heatmap", "source": "v", "source-layer": "hm", "paint": {"heatmap-radius": 24}}
    ]));
    let view = make_view(0.0, 82.0, 1.0, 0.0, 0.0);
    let np_px = ResolvedFrameView::resolve(&style, &view)
        .expect("res")
        .globe_camera()
        .expect("globe")
        .location_to_screen(maplibre::coords::LatLon::new(90.0, 0.0), 0.0);
    let mut world = World::default();
    let tile = tc(0, 0, 2);
    index_tile(
        &mut world,
        tile,
        "v",
        vec![
            point_geom(200.0, 25.0, "hm", 1, "hot-pole"),
            point_geom(200.0, 3800.0, "hm", 2, "cold-south"),
        ],
    );
    let pt_px = project_tile_pt(&style, &view, tile, [200.0, 25.0], 0.0).expect("vis");
    let hits = query(
        &world,
        &style,
        &view,
        QueryGeometry::Box {
            min: [np_px.x.min(pt_px[0]) - 2.0, np_px.y.min(pt_px[1]) - 2.0],
            max: [np_px.x.max(pt_px[0]) + 2.0, np_px.y.max(pt_px[1]) + 2.0],
        },
    );
    assert_eq!(names(&hits), ["hot-pole"]);
}

// 14 & 15: Fill-extrusion pitched nadir discovery (Q1) + straddling globe horizon (Q5).
#[test]
fn c14_15_extrusion_sub_eye_nadir_and_horizon_clip() {
    let style = globe_style(serde_json::json!([
        {"id": "bldg", "type": "fill-extrusion", "source": "v", "source-layer": "bldg", "paint": {"fill-extrusion-height": 250000}}
    ]));
    let view = make_view(0.0, 10.0, 3.0, 55.0, 0.0);
    let mut world = World::default();
    // Tower south of view center leans northward into the screen center via Q1 sub-eye nadir expansion.
    let tile = tc(4, 4, 3);
    index_tile(
        &mut world,
        tile,
        "v",
        vec![square_poly(200.0, 200.0, 120.0, "bldg", 1, "spire")],
    );
    let roof_pt =
        project_tile_pt(&style, &view, tile, [200.0, 200.0], 250_000.0).expect("roof visible");
    let hits = query(&world, &style, &view, QueryGeometry::Point(roof_pt));
    assert_eq!(names(&hits), ["spire"]);

    // Case 15: Extrusion straddling globe limb (left half visible, right half behind globe horizon).
    let disc_view = make_view(0.0, 0.0, 0.5, 0.0, 0.0);
    let mut hw = World::default();
    index_tile(
        &mut hw,
        tc(0, 0, 0),
        "v",
        vec![rect_poly(
            2400.0,
            1850.0,
            3600.0,
            2250.0,
            "bldg",
            2,
            "limb-bldg",
        )],
    );
    let limb_pt = project_tile_pt(&style, &disc_view, tc(0, 0, 0), [2450.0, 2048.0], 50_000.0)
        .expect("limb edge visible");
    assert_eq!(
        names(&query(
            &hw,
            &style,
            &disc_view,
            QueryGeometry::Point(limb_pt)
        )),
        ["limb-bldg"]
    );
}

// 16: Fill / line / circle translate at high latitude (60 deg N) matches shader translate units (Q7).
#[test]
fn c16_high_latitude_translate_parity() {
    let style = globe_style(serde_json::json!([
        {"id": "tr_fill", "type": "fill", "source": "v", "source-layer": "tr", "paint": {"fill-translate": [30, 0]}},
        {"id": "tr_circle", "type": "circle", "source": "v", "source-layer": "tr_pt", "paint": {"circle-radius": 6, "circle-translate": [30, 0], "circle-pitch-alignment": "map"}}
    ]));
    // At zoom 3, lon=0, lat=60 -> cos(60) = 0.5 (2x globe_scale distortion!)
    let view = make_view(22.5, 60.0, 3.0, 0.0, 0.0);
    let tile = tc(4, 2, 3);
    let mut world = World::default();
    index_tile(
        &mut world,
        tile,
        "v",
        vec![
            square_poly(1500.0, 2000.0, 60.0, "tr", 1, "tr-poly"),
            IndexedGeometry::from_point(
                geo_types::Point::new(1500.0, 2800.0),
                meta("tr_pt", 2, "tr-dot"),
            )
            .expect("pt"),
        ],
    );
    let base_units_per_px = EXTENT * view.zoom().scale_to_zoom_level(tile.z) / TILE_SIZE;
    let dx_tile = 30.0 * base_units_per_px;
    let shifted_fill =
        project_tile_pt(&style, &view, tile, [1500.0 + dx_tile, 2000.0], 0.0).expect("vis");
    let orig_fill = project_tile_pt(&style, &view, tile, [1500.0, 2000.0], 0.0).expect("vis");
    assert_eq!(
        names(&query(
            &world,
            &style,
            &view,
            QueryGeometry::Point(shifted_fill)
        )),
        ["tr-poly"]
    );
    assert!(query(&world, &style, &view, QueryGeometry::Point(orig_fill)).is_empty());

    let shifted_dot =
        project_tile_pt(&style, &view, tile, [1500.0 + dx_tile, 2800.0], 0.0).expect("vis");
    assert_eq!(
        names(&query(
            &world,
            &style,
            &view,
            QueryGeometry::Point(shifted_dot)
        )),
        ["tr-dot"]
    );
}

// 17 & 18: Mixed-LOD tiles (z=2 + z=4) & high-zoom polar span > 256 grid cells (Q6).
#[test]
fn c17_18_mixed_lod_and_large_grid_tiles() {
    let style = fill_style();
    let view = make_view(2.0, -5.0, 4.0, 0.0, 0.0);
    let mut world = World::default();
    // Coarse z=2 western quadrant (1,2,2) visible alongside fine z=4 eastern tile (8,8,4).
    index_tile(
        &mut world,
        tc(1, 2, 2),
        "v",
        vec![square_poly(3950.0, 450.0, 120.0, "land", 1, "coarse-z2")],
    );
    index_tile(
        &mut world,
        tc(8, 8, 4),
        "v",
        vec![square_poly(1500.0, 1500.0, 300.0, "land", 2, "fine-z4")],
    );
    let hits = query(
        &world,
        &style,
        &view,
        QueryGeometry::Box {
            min: [0.0, 0.0],
            max: [VIEW_W as f64, VIEW_H as f64],
        },
    );
    assert!(
        names(&hits).contains(&"coarse-z2"),
        "coarse z2 missed in mixed LOD: {hits:?}"
    );
    assert!(
        names(&hits).contains(&"fine-z4"),
        "fine z4 missed in mixed LOD: {hits:?}"
    );

    // Case 18: High zoom polar view (grid > 256 cells at z=6) still discovers loaded z=2 polar tiles.
    let polar_z6 = make_view(0.0, 82.0, 6.0, 0.0, 0.0);
    let np_px = ResolvedFrameView::resolve(&style, &polar_z6)
        .expect("res")
        .globe_camera()
        .expect("globe")
        .location_to_screen(maplibre::coords::LatLon::new(90.0, 0.0), 0.0);
    let mut pw = World::default();
    index_tile(
        &mut pw,
        tc(0, 0, 2),
        "v",
        vec![rect_poly(0.0, 0.0, EXTENT, 20.0, "land", 3, "hi-z-pole")],
    );
    let p_hits = query(
        &pw,
        &style,
        &polar_z6,
        QueryGeometry::Box {
            min: [np_px.x - 100.0, np_px.y - 100.0],
            max: [np_px.x + 100.0, np_px.y + 100.0],
        },
    );
    assert_eq!(names(&p_hits), ["hi-z-pole"]);
}

// 19, 20, 21, 22: Layers/filter options, draw-order sorting, antimeridian dedup, polygon holes.
#[test]
fn c19_20_21_22_filters_order_dedup_and_holes() {
    let style = globe_style(serde_json::json!([
        {"id": "bot", "type": "fill", "source": "v", "source-layer": "a"},
        {"id": "top", "type": "fill", "source": "v", "source-layer": "b"}
    ]));
    let view = make_view(180.0, -15.0, 2.0, 0.0, 0.0);
    let tile = tc(3, 2, 2);
    let mut world = World::default();
    let ring = LineString::from(vec![
        (3500.0, 400.0),
        (4000.0, 400.0),
        (4000.0, 900.0),
        (3500.0, 900.0),
        (3500.0, 400.0),
    ]);
    let hole = LineString::from(vec![
        (3650.0, 550.0),
        (3850.0, 550.0),
        (3850.0, 750.0),
        (3650.0, 750.0),
        (3650.0, 550.0),
    ]);
    let donut = IndexedGeometry::from_polygon(
        Polygon::new(ring, vec![hole]),
        meta_kind("a", 1, "donut", "ring"),
    )
    .expect("donut");
    index_tile(
        &mut world,
        tile,
        "v",
        vec![
            donut,
            square_poly_kind(3550.0, 450.0, 40.0, "b", 2, "cap", "solid"),
        ],
    );

    // Case 22: Hole misses, ring hits.
    let in_hole = project_tile_pt(&style, &view, tile, [3750.0, 650.0], 0.0).expect("vis");
    assert!(query(&world, &style, &view, QueryGeometry::Point(in_hole)).is_empty());

    // Case 20 & 21: Topmost layer "cap" first before "donut", no duplicate features across box.
    let on_both = project_tile_pt(&style, &view, tile, [3550.0, 450.0], 0.0).expect("vis");
    assert_eq!(
        names(&query(&world, &style, &view, QueryGeometry::Point(on_both))),
        ["cap", "donut"]
    );

    // Case 19: QueryOptions layer + filter scoping.
    let filtered = query_rendered_features(
        &world,
        &style,
        &view,
        QueryGeometry::Point(on_both),
        &QueryOptions {
            layers: Some(vec!["bot".into()]),
            filter: Some(serde_json::json!(["==", ["get", "kind"], "ring"])),
        },
    )
    .expect("filtered");
    assert_eq!(names(&filtered), ["donut"]);
}

// 23, 24, 25, 26: Sky miss, high pitch/bearing, transition zoom (z=11.5), and end-to-end HeadlessMap GeoJSON parity.
#[test]
fn c23_24_25_26_sky_pitch_transition_and_headless_geojson() {
    let style = fill_style();
    let zoom_out = make_view(0.0, 0.0, 0.5, 0.0, 0.0);
    let mut world = World::default();
    index_tile(
        &mut world,
        tc(0, 0, 0),
        "v",
        vec![square_poly(2048.0, 2048.0, 200.0, "land", 1, "home")],
    );
    assert!(query(&world, &style, &zoom_out, QueryGeometry::Point([2.0, 2.0])).is_empty());

    // Case 24: High pitch + bearing on globe
    let pitched = make_view(15.0, -15.0, 2.0, 60.0, 45.0);
    let mut pw = World::default();
    let t = tc(2, 2, 2);
    index_tile(
        &mut pw,
        t,
        "v",
        vec![square_poly(600.0, 600.0, 250.0, "land", 1, "pitched")],
    );
    let p_win = project_tile_pt(&style, &pitched, t, [600.0, 600.0], 0.0).expect("vis");
    assert_eq!(
        names(&query(&pw, &style, &pitched, QueryGeometry::Point(p_win))),
        ["pitched"]
    );

    // Case 25: Transition zoom (0 < globe_transition < 1 at z=11.5)
    let trans = make_view(0.0, 0.0, 11.5, 20.0, 0.0);
    let res_t = ResolvedFrameView::resolve(&style, &trans).expect("res");
    assert!(res_t.globe_transition() > 0.0 && res_t.globe_transition() < 1.0);
    let tc_t = tc(1024, 1024, 11);
    let mut tw = World::default();
    index_tile(
        &mut tw,
        tc_t,
        "v",
        vec![square_poly(2048.0, 2048.0, 600.0, "land", 1, "mid-trans")],
    );
    let t_win = project_tile_pt(&style, &trans, tc_t, [2048.0, 2048.0], 0.0).expect("vis");
    assert_eq!(
        names(&query(&tw, &style, &trans, QueryGeometry::Point(t_win))),
        ["mid-trans"]
    );

    // Case 26: End-to-end HeadlessMap GeoJSON query parity between flat Mercator and globe
    let flat_style: maplibre::style::Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "center": [0.0, 0.0],
        "zoom": 2,
        "sources": {"gj": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}},
        "layers": [{"id": "fill", "type": "fill", "source": "gj"}]
    }))
    .expect("flat");
    let globe_gj_style: maplibre::style::Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "globe"},
        "center": [0.0, 0.0],
        "zoom": 2,
        "sources": {"gj": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}},
        "layers": [{"id": "fill", "type": "fill", "source": "gj"}]
    }))
    .expect("globe");
    let flat_map = headless_map_for_geojson(flat_style, 256, 256);
    let globe_map = headless_map_for_geojson(globe_gj_style, 256, 256);
    let f_hits = flat_map
        .query_rendered_features(
            QueryGeometry::Point([128.0, 128.0]),
            &QueryOptions::default(),
        )
        .expect("flat");
    let g_hits = globe_map
        .query_rendered_features(
            QueryGeometry::Point([128.0, 128.0]),
            &QueryOptions::default(),
        )
        .expect("globe");
    assert_eq!(names(&f_hits), ["gj-equator"]);
    assert_eq!(names(&g_hits), ["gj-equator"]);
}
