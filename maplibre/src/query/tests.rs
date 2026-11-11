use std::sync::Arc;

use cgmath::Deg;
use geo_types::{LineString, Point, Polygon};
use image::{Rgba, RgbaImage};
use rstar::RTree;

use super::*;
use crate::{
    coords::{WorldCoords, WorldTileCoords, Zoom, ZoomLevel, EXTENT, TILE_SIZE},
    io::geometry_index::{FeatureMeta, IndexedGeometry, TileIndex},
    projection::ProjectionType,
    render::{
        resolved_view::ResolvedFrameView,
        view_state::{CameraAuthority, NavigationMode},
    },
    sdf::query::{QueryGeometry, QueryOptions},
    style::expression::{FeatureProperties, FEATURE_STATE_PREFIX},
    terrain::{DemTile, DemTileComponent, LoadedDem},
    window::PhysicalSize,
};

#[test]
fn feature_state_is_not_reported_as_a_property() {
    let properties: FeatureProperties = [
        ("name".to_owned(), Value::String("Bucks".into())),
        (format!("{FEATURE_STATE_PREFIX}big"), Value::Bool(true)),
    ]
    .into_iter()
    .collect();

    let reported = source_properties(&properties);
    assert_eq!(
        reported.into_iter().collect::<Vec<_>>(),
        [("name".to_owned(), serde_json::json!("Bucks"))]
    );
}

fn tc(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

fn make_view(x: f64, y: f64, zoom: Zoom, pitch_deg: f64) -> ViewState {
    ViewState::new(
        PhysicalSize::new(800, 600).unwrap(),
        WorldCoords::at_ground(x, y),
        zoom,
        Deg(pitch_deg),
        Deg(36.87),
    )
}

fn meta(layer: &str, id: u64, name: &str) -> FeatureMeta {
    let mut props = FeatureProperties::new();
    props.insert("name".into(), Value::String(name.into()));
    FeatureMeta {
        properties: Arc::new(props),
        source_layer: Arc::from(layer),
        id: Some(id),
        feature_index: id as u32,
    }
}

fn square_poly(
    cx: f64,
    cy: f64,
    half: f64,
    layer: &str,
    id: u64,
    name: &str,
) -> IndexedGeometry<f64> {
    let ring = LineString::from(vec![
        (cx - half, cy - half),
        (cx + half, cy - half),
        (cx + half, cy + half),
        (cx - half, cy + half),
        (cx - half, cy - half),
    ]);
    IndexedGeometry::from_polygon(Polygon::new(ring, Vec::new()), meta(layer, id, name)).unwrap()
}

fn index_tile(
    world: &mut World,
    coords: WorldTileCoords,
    source: &str,
    geoms: Vec<IndexedGeometry<f64>>,
) {
    let _ = world.tiles.spawn_mut(coords);
    world.tiles.geometry_index.index_tile(
        &coords,
        Some(source.to_owned()),
        TileIndex::Spatial {
            tree: RTree::bulk_load(geoms),
        },
    );
}

fn tile_unit_to_world(coords: WorldTileCoords, [ux, uy]: [f64; 2], zoom: Zoom) -> [f64; 2] {
    let tile_px = TILE_SIZE / zoom.scale_to_zoom_level(coords.z);
    [
        (coords.x as f64 + ux / EXTENT) * tile_px,
        (coords.y as f64 + uy / EXTENT) * tile_px,
    ]
}

fn names_of(hits: &[QueriedFeature]) -> Vec<&str> {
    hits.iter()
        .map(|f| f.properties["name"].as_str().unwrap())
        .collect()
}

fn query_at(
    world: &World,
    style: &Style,
    view: &ViewState,
    geom: QueryGeometry,
) -> Vec<QueriedFeature> {
    query_rendered_features(world, style, view, geom, &QueryOptions::default()).unwrap()
}

fn globe_vector_style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "globe"},
        "sources": {"v": {"type": "vector"}},
        "layers": [
            {"id": "land", "type": "fill", "source": "v", "source-layer": "land"},
            {"id": "poi", "type": "circle", "source": "v", "source-layer": "poi", "paint": {"circle-radius": 8}},
            {"id": "tower", "type": "fill-extrusion", "source": "v", "source-layer": "tower", "paint": {"fill-extrusion-height": 50000}}
        ]
    }))
    .unwrap()
}

#[test]
fn globe_queries_hit_front_hemisphere_occlude_antipode_and_miss_space() {
    let style = globe_vector_style();
    let zoom = Zoom::new(2.0);
    let world_size = TILE_SIZE * 2.0_f64.powf(zoom.value());
    let view = make_view(world_size * 0.5, world_size * 0.5, zoom, 0.0);
    let mut world = World::default();

    // Front hemisphere tile (2, 2, z=2) near (lon ~ +15 deg, lat ~ -15 deg).
    let front_tile = tc(2, 2, 2);
    let front_poly = square_poly(600.0, 600.0, 250.0, "land", 1, "front-land");
    let front_poi =
        IndexedGeometry::from_point(Point::new(600.0, 600.0), meta("poi", 2, "front-poi")).unwrap();
    let front_tower = square_poly(600.0, 600.0, 200.0, "tower", 3, "front-tower");
    index_tile(
        &mut world,
        front_tile,
        "v",
        vec![front_poly, front_poi, front_tower],
    );

    // Back-hemisphere antipodal tile (0, 1, z=2) near (lon ~ -165 deg, lat ~ +15 deg).
    let back_tile = tc(0, 1, 2);
    let back_poly = square_poly(600.0, 3496.0, 400.0, "land", 10, "antipode-land");
    let back_poi =
        IndexedGeometry::from_point(Point::new(600.0, 3496.0), meta("poi", 11, "antipode-poi"))
            .unwrap();
    let back_tower = square_poly(600.0, 3496.0, 400.0, "tower", 12, "antipode-tower");
    index_tile(
        &mut world,
        back_tile,
        "v",
        vec![back_poly, back_poi, back_tower],
    );

    let resolved = ResolvedFrameView::resolve(&style, &view).unwrap();
    let front_world = tile_unit_to_world(front_tile, [600.0, 600.0], zoom);
    let back_world = tile_unit_to_world(back_tile, [600.0, 3496.0], zoom);
    let front_px = resolved
        .project_world_point(front_world, 0.0)
        .expect("front feature is on visible hemisphere")
        .window;
    assert!(
        resolved.project_world_point(back_world, 0.0).is_none(),
        "antipode must be occluded behind globe horizon"
    );

    let hits = query_at(&world, &style, &view, QueryGeometry::Point(front_px));
    assert_eq!(names_of(&hits), ["front-tower", "front-poi", "front-land"]);

    let space_hits = query_at(&world, &style, &view, QueryGeometry::Point([4.0, 4.0]));
    assert!(
        space_hits.is_empty(),
        "outer space must miss: {space_hits:?}"
    );

    let all_hits = query_at(
        &world,
        &style,
        &view,
        QueryGeometry::Box {
            min: [0.0, 0.0],
            max: [800.0, 600.0],
        },
    );
    assert_eq!(
        names_of(&all_hits),
        ["front-tower", "front-poi", "front-land"],
        "antipode features must stay occluded even under full-viewport box: {all_hits:?}"
    );
}

#[test]
fn globe_antimeridian_box_query_finds_features_across_180_meridian() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "globe"},
        "sources": {"v": {"type": "vector"}},
        "layers": [{"id": "land", "type": "fill", "source": "v", "source-layer": "land"}]
    }))
    .unwrap();
    let zoom = Zoom::new(2.0);
    let world_size = TILE_SIZE * 2.0_f64.powf(zoom.value());
    let view = make_view(world_size, world_size * 0.5, zoom, 0.0);
    let mut world = World::default();
    index_tile(
        &mut world,
        tc(3, 2, 2),
        "v",
        vec![square_poly(3800.0, 300.0, 200.0, "land", 1, "fiji-west")],
    );
    index_tile(
        &mut world,
        tc(0, 2, 2),
        "v",
        vec![square_poly(300.0, 300.0, 200.0, "land", 2, "samoa-east")],
    );

    let hits = query_at(
        &world,
        &style,
        &view,
        QueryGeometry::Box {
            min: [340.0, 260.0],
            max: [460.0, 360.0],
        },
    );
    let mut names = names_of(&hits);
    names.sort_unstable();
    assert_eq!(names, ["fiji-west", "samoa-east"]);
}

#[test]
fn free_globe_pose_and_mercator_transition_queries_hit_projected_features() {
    let globe_style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": "vertical-perspective"},
        "sources": {"v": {"type": "vector"}},
        "layers": [{"id": "land", "type": "fill", "source": "v", "source-layer": "land"}]
    }))
    .unwrap();
    let zoom = Zoom::new(3.0);
    let world_size = TILE_SIZE * 2.0_f64.powf(zoom.value());
    let mut posed_view = make_view(world_size * 0.5, world_size * 0.5, zoom, 0.0);
    posed_view
        .set_navigation_mode(
            NavigationMode::FreeGlobe,
            &ProjectionType::VerticalPerspective,
        )
        .unwrap();
    let initial_pose = posed_view.globe_pose().unwrap();
    posed_view
        .orbit_globe_pose(initial_pose, cgmath::Rad(0.35), cgmath::Rad(0.4))
        .unwrap();
    assert_eq!(posed_view.camera_authority(), CameraAuthority::FreeGlobe);

    let mut world = World::default();
    let tile_z3 = tc(4, 3, 3);
    index_tile(
        &mut world,
        tile_z3,
        "v",
        vec![square_poly(900.0, 500.0, 300.0, "land", 1, "mediterranean")],
    );
    let resolved_posed = ResolvedFrameView::resolve(&globe_style, &posed_view).unwrap();
    let feat_world = tile_unit_to_world(tile_z3, [900.0, 500.0], zoom);
    let win = resolved_posed
        .project_world_point(feat_world, 0.0)
        .expect("visible under FreeGlobe pose")
        .window;
    let posed_hits = query_at(&world, &globe_style, &posed_view, QueryGeometry::Point(win));
    assert_eq!(names_of(&posed_hits), ["mediterranean"]);

    // Zoom 11.5 Mercator-to-Globe transition (0 < transition < 1).
    let trans_style = globe_vector_style();
    let trans_zoom = Zoom::new(11.5);
    let trans_ws = TILE_SIZE * 2.0_f64.powf(trans_zoom.value());
    let trans_tile = tc(1060, 715, 11);
    let center = tile_unit_to_world(trans_tile, [2048.0, 2048.0], trans_zoom);
    let trans_view = make_view(center[0], center[1], trans_zoom, 35.0);
    let mut trans_world = World::default();
    index_tile(
        &mut trans_world,
        trans_tile,
        "v",
        vec![
            square_poly(3200.0, 1200.0, 180.0, "land", 5, "blended-parcel"),
            IndexedGeometry::from_point(Point::new(3200.0, 1200.0), meta("poi", 6, "blended-poi"))
                .unwrap(),
        ],
    );
    let resolved_trans = ResolvedFrameView::resolve(&trans_style, &trans_view).unwrap();
    assert!(resolved_trans.globe_transition() > 0.0 && resolved_trans.globe_transition() < 1.0);
    let parcel_world = tile_unit_to_world(trans_tile, [3200.0, 1200.0], trans_zoom);
    assert!(parcel_world[0] < trans_ws && parcel_world[1] < trans_ws);
    let blended_px = resolved_trans
        .project_world_point(parcel_world, 0.0)
        .unwrap()
        .window;
    let blended_hits = query_at(
        &trans_world,
        &trans_style,
        &trans_view,
        QueryGeometry::Point(blended_px),
    );
    assert_eq!(names_of(&blended_hits), ["blended-poi", "blended-parcel"]);
}

#[test]
fn terrain_pitched_query_hits_elevated_ridge_extrusion_and_circle_and_misses_sky() {
    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {
            "dem": {
                "type": "raster-dem",
                "tiles": ["https://example.invalid/{z}/{x}/{y}.png"],
                "encoding": "terrarium",
                "tileSize": 64,
                "maxzoom": 12
            },
            "v": {"type": "vector"}
        },
        "terrain": {"source": "dem", "exaggeration": 1.0},
        "layers": [
            {"id": "ridge", "type": "fill", "source": "v", "source-layer": "ridge"},
            {"id": "hut-marker", "type": "circle", "source": "v", "source-layer": "hut", "paint": {"circle-radius": 6}},
            {"id": "alpine-tower", "type": "fill-extrusion", "source": "v", "source-layer": "tower", "paint": {"fill-extrusion-height": 200}}
        ]
    }))
    .unwrap();

    let zoom = Zoom::new(12.0);
    let tile = tc(2130, 1435, 12);
    let center = tile_unit_to_world(tile, [2048.0, 2048.0], zoom);
    let mut view = make_view(center[0], center[1], zoom, 65.0);
    view.set_center_elevation(1500.0);

    let unpack = [256.0, 1.0, 1.0 / 256.0, 32768.0];
    let encoded = 1500.0_f64 + 32768.0;
    let r = (encoded / 256.0).floor() as u8;
    let g = (encoded - f64::from(r) * 256.0).floor() as u8;
    let dem_img = RgbaImage::from_pixel(64, 64, Rgba([r, g, 0, 255]));
    let dem_tile = DemTile::from_image(&dem_img, unpack).unwrap();

    let mut world = World::default();
    for dy in -2..=2 {
        for dx in -2..=2 {
            let c = tc(tile.x + dx, tile.y + dy, 12);
            let _ = world.tiles.spawn_mut(c);
            if let Some(dem_c) = c.get_parent() {
                world
                    .tiles
                    .spawn_mut(dem_c)
                    .unwrap()
                    .insert(DemTileComponent::Loaded(LoadedDem::new(dem_tile.clone())));
            }
        }
    }

    let target_uv = [2048.0, 1200.0];
    index_tile(
        &mut world,
        tile,
        "v",
        vec![
            square_poly(
                target_uv[0],
                target_uv[1],
                120.0,
                "ridge",
                1,
                "ridge-plateau",
            ),
            IndexedGeometry::from_point(
                Point::new(target_uv[0], target_uv[1]),
                meta("hut", 2, "alpine-hut"),
            )
            .unwrap(),
            square_poly(target_uv[0], target_uv[1], 80.0, "tower", 3, "summit-tower"),
        ],
    );

    let resolved = ResolvedFrameView::resolve(&style, &view).unwrap();
    let target_world = tile_unit_to_world(tile, target_uv, zoom);
    let elevated_px = resolved
        .project_world_point(target_world, 1500.0)
        .unwrap()
        .window;
    let sea_level_px = resolved
        .project_world_point(target_world, 0.0)
        .unwrap()
        .window;
    assert!((elevated_px[1] - sea_level_px[1]).abs() > 40.0);

    let surface_hits = query_at(&world, &style, &view, QueryGeometry::Point(elevated_px));
    assert_eq!(
        names_of(&surface_hits),
        ["summit-tower", "alpine-hut", "ridge-plateau"]
    );

    let far_edge_world = tile_unit_to_world(tile, [target_uv[0], target_uv[1] - 80.0], zoom);
    let tower_roof_px = resolved
        .project_world_point(far_edge_world, 1680.0)
        .unwrap()
        .window;
    let tower_hits = query_at(&world, &style, &view, QueryGeometry::Point(tower_roof_px));
    assert!(names_of(&tower_hits).contains(&"summit-tower"));

    let sky_hits = query_at(&world, &style, &view, QueryGeometry::Point([400.0, 2.0]));
    assert!(
        sky_hits.is_empty(),
        "sky above horizon must miss: {sky_hits:?}"
    );
}
