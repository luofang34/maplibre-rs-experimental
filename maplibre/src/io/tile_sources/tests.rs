#![allow(clippy::expect_used, clippy::panic)]

use super::{
    clamp_to_max_zoom, covering_zoom, raster_zoom_delta, source_layer_groups, source_max_zoom,
    source_tile_size, source_tiles_for, TileKind,
};
use crate::{
    coords::{WorldTileCoords, ZoomLevel},
    io::source_type::SourceType,
    style::Style,
};

fn style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {
            "world": {"type": "vector", "tiles": ["https://w.example/{z}/{x}/{y}.pbf"], "maxzoom": 6},
            "detail": {"type": "vector", "tiles": ["https://d.example/{z}/{x}/{y}.pbf"], "maxzoom": 14, "scheme": "tms"},
            "photo": {"type": "raster", "tiles": ["https://p.example/{z}/{x}/{y}.jpg"], "maxzoom": 3},
            "shapes": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}},
            "catalog": {"type": "vector", "url": "https://c.example/tiles.json"}
        },
        "layers": [
            {"id": "bg", "type": "background", "paint": {"background-color": "white"}},
            {"id": "land", "type": "fill", "source": "world", "source-layer": "countries", "paint": {"fill-color": "red"}},
            {"id": "roads", "type": "line", "source": "detail", "source-layer": "roads", "paint": {"line-color": "red"}},
            {"id": "borders", "type": "line", "source": "world", "source-layer": "countries", "paint": {"line-color": "red"}},
            {"id": "sat", "type": "raster", "source": "photo"},
            {"id": "geo", "type": "fill", "source": "shapes", "paint": {"fill-color": "red"}},
            {"id": "legacy", "type": "fill", "source-layer": "water", "paint": {"fill-color": "red"}},
            {"id": "catalogued", "type": "fill", "source": "catalog", "source-layer": "x", "paint": {"fill-color": "red"}}
        ]
    }))
    .expect("style parses")
}

fn coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    WorldTileCoords {
        x,
        y,
        z: ZoomLevel::new(z),
    }
}

#[test]
fn vector_layers_group_by_source_template() {
    let groups = source_layer_groups(&style(), TileKind::Vector);
    let names: Vec<_> = groups.iter().map(|g| g.source_name.clone()).collect();

    assert_eq!(
        names,
        vec![None, Some("detail".into()), Some("world".into())]
    );
    let world = groups
        .iter()
        .find(|g| g.source_name.as_deref() == Some("world"))
        .expect("world group");
    let ids: Vec<_> = world.layers.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, vec!["land", "borders"]);
    let SourceType::Tessellate(source) = &world.source else {
        panic!("vector groups fetch tessellate sources");
    };
    assert_eq!(
        source.format(&coords(1, 0, 1)).expect("in range"),
        "https://w.example/1/1/0.pbf"
    );
}

#[test]
fn tms_scheme_reaches_the_template_source() {
    let groups = source_layer_groups(&style(), TileKind::Vector);
    let detail = groups
        .iter()
        .find(|g| g.source_name.as_deref() == Some("detail"))
        .expect("detail group");

    assert_eq!(
        detail.source.format(&coords(0, 0, 1)).expect("in range"),
        "https://d.example/1/0/1.pbf"
    );
}

#[test]
fn unresolvable_sources_fall_back_to_the_default_group() {
    let groups = source_layer_groups(&style(), TileKind::Vector);
    let fallback = groups
        .iter()
        .find(|g| g.source_name.is_none())
        .expect("default group");
    let ids: Vec<_> = fallback.layers.iter().map(|l| l.id.as_str()).collect();

    assert_eq!(ids, vec!["legacy", "catalogued"]);
    assert!(fallback
        .source
        .format(&coords(0, 0, 0))
        .expect("in range")
        .starts_with("https://maps.tuerantuer.org/europe_germany/0/0/0"));
}

#[test]
fn geojson_and_background_layers_are_not_tile_groups() {
    let groups = source_layer_groups(&style(), TileKind::Vector);

    assert!(groups
        .iter()
        .flat_map(|g| &g.layers)
        .all(|l| l.id != "geo" && l.id != "bg"));
}

#[test]
fn raster_layers_group_separately() {
    let groups = source_layer_groups(&style(), TileKind::Raster);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].source_name.as_deref(), Some("photo"));
    assert!(matches!(groups[0].source, SourceType::Raster(_)));
}

#[test]
fn max_zoom_is_the_most_restrictive_source() {
    assert_eq!(source_max_zoom(&style(), TileKind::Vector), Some(6));
    assert_eq!(source_max_zoom(&style(), TileKind::Raster), Some(3));
}

#[test]
fn clamping_walks_to_the_ancestor_at_max_zoom() {
    assert_eq!(
        clamp_to_max_zoom(coords(37, 21, 6), Some(3)),
        coords(4, 2, 3)
    );
    assert_eq!(
        clamp_to_max_zoom(coords(37, 21, 6), Some(6)),
        coords(37, 21, 6)
    );
    assert_eq!(
        clamp_to_max_zoom(coords(37, 21, 6), None),
        coords(37, 21, 6)
    );
}

#[test]
fn covering_zoom_rounds_raster_sources_after_the_tile_size_adjustment() {
    assert_eq!(covering_zoom(12.0, TileKind::Raster, 256.0), 13);
    assert_eq!(covering_zoom(12.4, TileKind::Raster, 256.0), 13);
    assert_eq!(covering_zoom(12.6, TileKind::Raster, 256.0), 14);
    assert_eq!(covering_zoom(12.6, TileKind::Raster, 512.0), 13);
    assert_eq!(covering_zoom(-0.5, TileKind::Raster, 256.0), 1);
    assert_eq!(covering_zoom(12.6, TileKind::Vector, 512.0), 12);
}

#[test]
fn source_tiles_follow_the_zoom_delta_within_the_source_range() {
    let view = coords(6, 4, 5);
    let children = source_tiles_for(view, 1, None, None);
    assert_eq!(children, view.get_children().to_vec());
    assert_eq!(source_tiles_for(view, 0, None, None), vec![view]);
    assert_eq!(
        source_tiles_for(view, -1, None, None),
        vec![coords(3, 2, 4)]
    );
    assert_eq!(
        source_tiles_for(view, 1, None, Some(5)),
        vec![view],
        "capped at max zoom"
    );
    assert!(
        source_tiles_for(view, 0, Some(6), None).is_empty(),
        "nothing below min zoom"
    );
    assert_eq!(source_tiles_for(view, 2, None, None).len(), 16);
    assert_eq!(
        source_tiles_for(view, 3, None, None).len(),
        16,
        "descent is bounded"
    );
}

#[test]
fn raster_zoom_delta_follows_the_smallest_raster_tile_size_unless_vectors_share_the_view() {
    let raster_only: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {
            "photo": {"type": "raster", "tiles": ["https://p.example/{z}/{x}/{y}.jpg"], "tileSize": 256}
        },
        "layers": [{"id": "sat", "type": "raster", "source": "photo"}]
    }))
    .expect("style parses");

    assert_eq!(source_tile_size(&raster_only, TileKind::Raster), 256.0);
    assert_eq!(raster_zoom_delta(&raster_only, 12.0), 1);
    assert_eq!(raster_zoom_delta(&raster_only, 12.6), 2);
    assert_eq!(source_tile_size(&style(), TileKind::Raster), 512.0);
    assert_eq!(
        raster_zoom_delta(&style(), 12.6),
        0,
        "vector tiles share the view"
    );
}
