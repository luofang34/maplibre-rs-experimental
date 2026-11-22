//! C3 Globe Query Conformance suite covering all 26 globe, polar, horizon, antimeridian,
//! translate, mixed-LOD, extrusion, circle, line, heatmap, and flat/transition query cases.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic)]

#[path = "globe_query/extra_cases.rs"]
mod extra_cases;
#[path = "globe_query/helpers.rs"]
mod h;

use h::*;
use maplibre::{
    coords::EXTENT, render::resolved_view::ResolvedFrameView, sdf::query::QueryGeometry,
    tcs::world::World,
};

fn fill_style() -> maplibre::style::Style {
    globe_style(serde_json::json!([
        {"id": "land", "type": "fill", "source": "v", "source-layer": "land"}
    ]))
}

// 1 & 2: Equator point query + box query on globe fill polygons.
#[test]
fn c01_02_equator_point_and_box_fill_queries() {
    let style = fill_style();
    for zoom in [1.0, 3.0] {
        let view = make_view(5.0, -5.0, zoom, 0.0, 0.0);
        let z_tile = zoom as u8;
        let coords = if z_tile == 1 {
            tc(1, 1, 1)
        } else {
            tc(4, 4, 3)
        };
        let mut world = World::default();
        index_tile(
            &mut world,
            coords,
            "v",
            vec![square_poly(600.0, 600.0, 250.0, "land", 1, "eq")],
        );
        let pt = project_tile_pt(&style, &view, coords, [600.0, 600.0], 0.0).expect("visible");
        assert_eq!(
            names(&query(&world, &style, &view, QueryGeometry::Point(pt))),
            ["eq"]
        );
        assert_eq!(
            names(&query(
                &world,
                &style,
                &view,
                QueryGeometry::Box {
                    min: [pt[0] - 8.0, pt[1] - 8.0],
                    max: [pt[0] + 8.0, pt[1] + 8.0],
                }
            )),
            ["eq"]
        );
    }
}

// 3: Back-of-globe antipode occlusion under both point and full-viewport box queries.
#[test]
fn c03_back_of_globe_antipode_is_occluded() {
    let style = fill_style();
    let view = make_view(15.0, -15.0, 2.0, 0.0, 0.0);
    let mut world = World::default();
    let front = tc(2, 2, 2);
    let back = tc(0, 1, 2);
    index_tile(
        &mut world,
        front,
        "v",
        vec![square_poly(600.0, 600.0, 200.0, "land", 1, "front")],
    );
    index_tile(
        &mut world,
        back,
        "v",
        vec![square_poly(600.0, 3496.0, 400.0, "land", 2, "back")],
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
    assert_eq!(names(&hits), ["front"]);
}

// 4: Antimeridian-crossing features & box query at +/-180 deg.
#[test]
fn c04_antimeridian_crossing_fill_query() {
    let style = fill_style();
    let view = make_view(180.0, -15.0, 2.0, 0.0, 0.0);
    let mut world = World::default();
    index_tile(
        &mut world,
        tc(3, 2, 2),
        "v",
        vec![square_poly(3900.0, 400.0, 180.0, "land", 1, "west")],
    );
    index_tile(
        &mut world,
        tc(0, 2, 2),
        "v",
        vec![square_poly(200.0, 400.0, 180.0, "land", 2, "east")],
    );
    let hits = query(
        &world,
        &style,
        &view,
        QueryGeometry::Box {
            min: [340.0, 220.0],
            max: [460.0, 380.0],
        },
    );
    assert_eq!(names(&hits), ["west", "east"]);
}

// 5 & 6: North Pole and South Pole enclosing box queries (Q2).
#[test]
fn c05_06_north_and_south_pole_enclosing_box_queries() {
    let style = fill_style();
    let north_view = make_view(0.0, 82.0, 1.0, 0.0, 0.0);
    let north_res = ResolvedFrameView::resolve(&style, &north_view).expect("res");
    let np_px = north_res
        .globe_camera()
        .expect("globe")
        .location_to_screen(maplibre::coords::LatLon::new(90.0, 0.0), 0.0);
    let mut nw = World::default();
    for tx in 0..4 {
        index_tile(
            &mut nw,
            tc(tx, 0, 2),
            "v",
            vec![rect_poly(
                0.0,
                0.0,
                EXTENT,
                40.0,
                "land",
                (tx + 1) as u64,
                &format!("np-{tx}"),
            )],
        );
    }
    index_tile(
        &mut nw,
        tc(2, 1, 2),
        "v",
        vec![square_poly(2048.0, 2048.0, 200.0, "land", 99, "midlat")],
    );
    let n_hits = query(
        &nw,
        &style,
        &north_view,
        QueryGeometry::Box {
            min: [np_px.x - 60.0, np_px.y - 60.0],
            max: [np_px.x + 60.0, np_px.y + 60.0],
        },
    );
    assert_eq!(
        n_hits.len(),
        4,
        "all 4 polar quadrant tiles should hit around North Pole: {n_hits:?}"
    );
    assert!(!names(&n_hits).contains(&"midlat"));

    let south_view = make_view(0.0, -82.0, 1.0, 0.0, 0.0);
    let south_res = ResolvedFrameView::resolve(&style, &south_view).expect("res");
    let sp_px = south_res
        .globe_camera()
        .expect("globe")
        .location_to_screen(maplibre::coords::LatLon::new(-90.0, 0.0), 0.0);
    let mut sw = World::default();
    for tx in 0..4 {
        index_tile(
            &mut sw,
            tc(tx, 3, 2),
            "v",
            vec![rect_poly(
                0.0,
                EXTENT - 40.0,
                EXTENT,
                EXTENT,
                "land",
                (tx + 10) as u64,
                &format!("sp-{tx}"),
            )],
        );
    }
    let s_hits = query(
        &sw,
        &style,
        &south_view,
        QueryGeometry::Box {
            min: [sp_px.x - 60.0, sp_px.y - 60.0],
            max: [sp_px.x + 60.0, sp_px.y + 60.0],
        },
    );
    assert_eq!(
        s_hits.len(),
        4,
        "all 4 polar quadrant tiles should hit around South Pole: {s_hits:?}"
    );
}

// 7 & 8: Zoomed-out whole globe disc box (z=0.5) + large polygon crossing the globe horizon (Q3).
#[test]
fn c07_08_whole_globe_disc_and_horizon_crossing_polygon() {
    let style = fill_style();
    let view = make_view(0.0, 0.0, 0.5, 0.0, 0.0);
    let mut world = World::default();
    // Large polygon with right corners past globe limb (+130° lon) and left corners visible (+10° lon).
    let front_cross = rect_poly(2160.0, 1700.0, 3600.0, 2400.0, "land", 1, "cross-horizon");
    let antipode = square_poly(100.0, 2048.0, 120.0, "land", 2, "far-side");
    index_tile(&mut world, tc(0, 0, 0), "v", vec![front_cross, antipode]);

    let full_hits = query(
        &world,
        &style,
        &view,
        QueryGeometry::Box {
            min: [0.0, 0.0],
            max: [VIEW_W as f64, VIEW_H as f64],
        },
    );
    assert_eq!(names(&full_hits), ["cross-horizon"]);

    // Giant polygon spanning front & back whose 4 corners are outside the sub-box, tested under zoomed-out disc box!
    let mut gigantic = World::default();
    index_tile(
        &mut gigantic,
        tc(0, 0, 0),
        "v",
        vec![rect_poly(512.0, 1600.0, 3584.0, 2500.0, "land", 3, "giant")],
    );
    let center_box = query(
        &gigantic,
        &style,
        &view,
        QueryGeometry::Box {
            min: [0.0, 0.0],
            max: [VIEW_W as f64, VIEW_H as f64],
        },
    );
    assert_eq!(names(&center_box), ["giant"]);
}

// 9 & 10: Wide line-width + line-offset + line crossing globe horizon under whole_visible_hemisphere (Q3).
#[test]
fn c09_10_wide_offset_line_and_horizon_crossing_line() {
    let style = globe_style(serde_json::json!([
        {"id": "road", "type": "line", "source": "v", "source-layer": "road", "paint": {"line-width": 20, "line-offset": 12}}
    ]));
    // Zoom 2 near equator
    let view = make_view(20.0, -20.0, 2.0, 0.0, 0.0);
    let mut world = World::default();
    let tile = tc(2, 2, 2);
    index_tile(
        &mut world,
        tile,
        "v",
        vec![line_geom(
            vec![(1200.0, 2000.0), (2200.0, 2000.0)],
            "road",
            1,
            "ave",
        )],
    );
    let line_pt = project_tile_pt(&style, &view, tile, [1700.0, 2000.0], 0.0).expect("line pt");
    // line-offset shifts right of direction (+y on screen), so +14px y hits, -15px y misses!
    assert_eq!(
        names(&query(
            &world,
            &style,
            &view,
            QueryGeometry::Point([line_pt[0], line_pt[1] + 14.0])
        )),
        ["ave"]
    );
    assert!(query(
        &world,
        &style,
        &view,
        QueryGeometry::Point([line_pt[0], line_pt[1] - 15.0])
    )
    .is_empty());

    // Zoomed-out globe with line crossing from front (+10°) behind eastern limb (+140°)
    let disc_view = make_view(0.0, 0.0, 0.5, 0.0, 0.0);
    let mut dw = World::default();
    index_tile(
        &mut dw,
        tc(0, 0, 0),
        "v",
        vec![line_geom(
            vec![(2150.0, 2048.0), (3650.0, 2048.0)],
            "road",
            2,
            "limb-road",
        )],
    );
    let disc_hits = query(
        &dw,
        &style,
        &disc_view,
        QueryGeometry::Box {
            min: [0.0, 0.0],
            max: [VIEW_W as f64, VIEW_H as f64],
        },
    );
    assert_eq!(names(&disc_hits), ["limb-road"]);
}

// 11 & 12: Circle viewport vs map pitch alignment on pitched globe & during whole_visible_hemisphere (Q4).
#[test]
fn c11_12_circle_alignment_modes_on_globe_and_whole_hemisphere() {
    let vp_style = globe_style(serde_json::json!([
        {"id": "c_vp", "type": "circle", "source": "v", "source-layer": "pts", "paint": {"circle-radius": 18, "circle-pitch-alignment": "viewport", "circle-pitch-scale": "map"}}
    ]));
    let map_style = globe_style(serde_json::json!([
        {"id": "c_map", "type": "circle", "source": "v", "source-layer": "pts", "paint": {"circle-radius": 18, "circle-pitch-alignment": "map", "circle-pitch-scale": "viewport"}}
    ]));
    let view = make_view(15.0, -15.0, 2.0, 45.0, 0.0);
    let tile = tc(2, 2, 2);
    let mut world = World::default();
    index_tile(
        &mut world,
        tile,
        "v",
        vec![
            point_geom(1000.0, 1000.0, "pts", 1, "hit-pt"),
            point_geom(3500.0, 3500.0, "pts", 2, "far-pt"),
        ],
    );
    let p_hit = project_tile_pt(&vp_style, &view, tile, [1000.0, 1000.0], 0.0).expect("hit pt");
    assert_eq!(
        names(&query(
            &world,
            &vp_style,
            &view,
            QueryGeometry::Point([p_hit[0] + 10.0, p_hit[1]])
        )),
        ["hit-pt"]
    );
    assert_eq!(
        names(&query(
            &world,
            &map_style,
            &view,
            QueryGeometry::Point([p_hit[0] + 10.0, p_hit[1]])
        )),
        ["hit-pt"]
    );

    // Whole visible hemisphere over North Pole: only the circle inside the box hits (no false positive on sub-pole!).
    let pole_view = make_view(0.0, 82.0, 1.0, 0.0, 0.0);
    let np_px = ResolvedFrameView::resolve(&map_style, &pole_view)
        .expect("res")
        .globe_camera()
        .expect("globe")
        .location_to_screen(maplibre::coords::LatLon::new(90.0, 0.0), 0.0);
    let mut pw = World::default();
    index_tile(
        &mut pw,
        tc(0, 0, 2),
        "v",
        vec![
            point_geom(200.0, 30.0, "pts", 3, "near-pole"),
            point_geom(200.0, 3900.0, "pts", 4, "sub-pole"),
        ],
    );
    let pt_px =
        project_tile_pt(&map_style, &pole_view, tc(0, 0, 2), [200.0, 30.0], 0.0).expect("vis");
    let pole_hits = query(
        &pw,
        &map_style,
        &pole_view,
        QueryGeometry::Box {
            min: [np_px.x.min(pt_px[0]) - 2.0, np_px.y.min(pt_px[1]) - 2.0],
            max: [np_px.x.max(pt_px[0]) + 2.0, np_px.y.max(pt_px[1]) + 2.0],
        },
    );
    assert_eq!(names(&pole_hits), ["near-pole"]);
}
