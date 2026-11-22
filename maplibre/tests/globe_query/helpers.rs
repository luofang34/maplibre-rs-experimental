use std::{f64::consts::TAU, sync::Arc};

use cgmath::Deg;
use geo_types::{LineString, Point, Polygon};
use maplibre::{
    background::BackgroundPlugin,
    coords::{LatLon, WorldCoords, WorldTileCoords, Zoom, ZoomLevel, EXTENT, TILE_SIZE},
    headless::{
        create_headless_renderer_with_settings,
        map::{process_geojson_layers, HeadlessMap},
        HeadlessPlugin,
    },
    io::geometry_index::{FeatureMeta, IndexedGeometry, TileIndex},
    plugin::Plugin,
    projection::ProjectionType,
    query::{query_rendered_features, QueriedFeature},
    render::{
        resolved_view::ResolvedFrameView,
        settings::{Msaa, RendererSettings},
        view_state::ViewState,
        RenderPlugin,
    },
    sdf::query::{QueryGeometry, QueryOptions},
    style::{
        expression::{FeatureProperties, Value},
        Style,
    },
    tcs::world::World,
    vector::{DefaultVectorTransferables, VectorPlugin},
    window::PhysicalSize,
};
use rstar::RTree;

pub const VIEW_W: u32 = 800;
pub const VIEW_H: u32 = 600;
const MERCATOR_LAT_LIMIT: f64 = 85.051_128_779_806_59;

pub fn tc(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

pub fn lat_lon_to_world(loc: LatLon, zoom: Zoom) -> [f64; 2] {
    let world_size = TILE_SIZE * 2.0_f64.powf(zoom.value());
    let mx = (loc.longitude + 180.0) / 360.0;
    let lat_rad = loc
        .latitude
        .clamp(-MERCATOR_LAT_LIMIT, MERCATOR_LAT_LIMIT)
        .to_radians();
    let my = (0.5 - lat_rad.tan().asinh() / TAU).clamp(0.0, 1.0);
    [mx * world_size, my * world_size]
}

pub fn make_view(lon: f64, lat: f64, zoom_val: f64, pitch_deg: f64, bearing_deg: f64) -> ViewState {
    let zoom = Zoom::new(zoom_val);
    let [wx, wy] = lat_lon_to_world(LatLon::new(lat, lon), zoom);
    let mut view = ViewState::new(
        PhysicalSize::new(VIEW_W, VIEW_H).expect("size"),
        WorldCoords::at_ground(wx, wy),
        zoom,
        Deg(pitch_deg),
        Deg(36.87),
    );
    if bearing_deg != 0.0 {
        view.camera_mut().set_bearing(Deg(bearing_deg));
    }
    view
}

pub fn tile_unit_to_world(coords: WorldTileCoords, [ux, uy]: [f64; 2], zoom: Zoom) -> [f64; 2] {
    let tile_px = TILE_SIZE / zoom.scale_to_zoom_level(coords.z);
    [
        (coords.x as f64 + ux / EXTENT) * tile_px,
        (coords.y as f64 + uy / EXTENT) * tile_px,
    ]
}

pub fn project_tile_pt(
    style: &Style,
    view: &ViewState,
    coords: WorldTileCoords,
    uv: [f64; 2],
    elev_m: f64,
) -> Option<[f64; 2]> {
    let resolved = ResolvedFrameView::resolve(style, view).ok()?;
    let wp = tile_unit_to_world(coords, uv, view.zoom());
    resolved.project_world_point(wp, elev_m).map(|p| p.window)
}

pub fn meta(layer: &str, id: u64, name: &str) -> FeatureMeta {
    let mut props = FeatureProperties::default();
    props.insert("name".into(), Value::String(name.into()));
    FeatureMeta {
        properties: Arc::new(props),
        source_layer: Arc::from(layer),
        id: Some(id),
        feature_index: id as u32,
    }
}

pub fn meta_kind(layer: &str, id: u64, name: &str, kind: &str) -> FeatureMeta {
    let mut m = meta(layer, id, name);
    Arc::make_mut(&mut m.properties).insert("kind".into(), Value::String(kind.into()));
    m
}

pub fn rect_poly(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    layer: &str,
    id: u64,
    name: &str,
) -> IndexedGeometry<f64> {
    let ring = LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]);
    IndexedGeometry::from_polygon(Polygon::new(ring, Vec::new()), meta(layer, id, name))
        .expect("poly")
}

pub fn square_poly(
    cx: f64,
    cy: f64,
    half: f64,
    layer: &str,
    id: u64,
    name: &str,
) -> IndexedGeometry<f64> {
    rect_poly(cx - half, cy - half, cx + half, cy + half, layer, id, name)
}

pub fn square_poly_kind(
    cx: f64,
    cy: f64,
    half: f64,
    layer: &str,
    id: u64,
    name: &str,
    kind: &str,
) -> IndexedGeometry<f64> {
    let ring = LineString::from(vec![
        (cx - half, cy - half),
        (cx + half, cy - half),
        (cx + half, cy + half),
        (cx - half, cy + half),
        (cx - half, cy - half),
    ]);
    IndexedGeometry::from_polygon(
        Polygon::new(ring, Vec::new()),
        meta_kind(layer, id, name, kind),
    )
    .expect("poly")
}

pub fn line_geom(pts: Vec<(f64, f64)>, layer: &str, id: u64, name: &str) -> IndexedGeometry<f64> {
    IndexedGeometry::from_linestring(LineString::from(pts), meta(layer, id, name)).expect("line")
}

pub fn point_geom(x: f64, y: f64, layer: &str, id: u64, name: &str) -> IndexedGeometry<f64> {
    IndexedGeometry::from_point(Point::new(x, y), meta(layer, id, name)).expect("pt")
}

pub fn index_tile(
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

pub fn query(
    world: &World,
    style: &Style,
    view: &ViewState,
    geom: QueryGeometry,
) -> Vec<QueriedFeature> {
    query_rendered_features(world, style, view, geom, &QueryOptions::default()).expect("query")
}

pub fn names(hits: &[QueriedFeature]) -> Vec<&str> {
    hits.iter()
        .map(|f| f.properties["name"].as_str().expect("name"))
        .collect()
}

pub fn style_with(projection: &str, layers: serde_json::Value) -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "projection": {"type": projection},
        "sources": {"v": {"type": "vector"}},
        "layers": layers
    }))
    .expect("style")
}

pub fn globe_style(layers: serde_json::Value) -> Style {
    style_with("globe", layers)
}

pub fn headless_map_for_geojson(style: Style, width: u32, height: u32) -> HeadlessMap {
    let settings = RendererSettings {
        msaa: Msaa { samples: 1 },
        ..Default::default()
    };
    let (kernel, renderer) = futures::executor::block_on(create_headless_renderer_with_settings(
        width, height, None, settings,
    ))
    .expect("headless renderer");
    let plugins: Vec<Box<dyn Plugin<_>>> = vec![
        Box::new(RenderPlugin),
        Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
        Box::new(BackgroundPlugin),
        Box::new(HeadlessPlugin::new(false)),
    ];
    let mut map = HeadlessMap::new(style, renderer, kernel, plugins).expect("headless map");
    let matching: Vec<_> = map
        .context()
        .style
        .layers
        .iter()
        .filter(|l| l.source.as_deref() == Some("gj"))
        .cloned()
        .collect();
    let gj = serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "id": 77,
            "properties": {"name": "gj-equator"},
            "geometry": {
                "type": "Polygon",
                "coordinates": [[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]]
            }
        }]
    });
    let proj = if map.context().style.projection.is_some() {
        ProjectionType::Globe
    } else {
        ProjectionType::Mercator
    };
    let processed = process_geojson_layers(&gj, "gj", matching, tc(0, 0, 0), proj).expect("gj");
    map.render_tile(processed).expect("render tile");
    map.run_frame().expect("frame");
    map
}
