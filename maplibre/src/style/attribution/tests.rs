#![allow(clippy::expect_used, clippy::panic)]

use crate::style::Style;

fn style() -> Style {
    serde_json::from_value(serde_json::json!({
        "version": 8,
        "terrain": {"source": "dem"},
        "sources": {
            "imagery": {"type": "raster", "tiles": ["https://i.example/{z}/{x}/{y}"],
                "attribution": "USGS NAIP"},
            "roads": {"type": "vector", "tiles": ["https://v.example/{z}/{x}/{y}"],
                "attribution": "© OpenStreetMap contributors"},
            "osm": {"type": "raster", "tiles": ["https://o.example/{z}/{x}/{y}"],
                "attribution": "OpenStreetMap"},
            "hidden": {"type": "raster", "tiles": ["https://h.example/{z}/{x}/{y}"],
                "attribution": "Esri World Imagery"},
            "dem": {"type": "raster-dem", "tiles": ["https://d.example/{z}/{x}/{y}"],
                "attribution": "Mapzen Terrain Tiles"}
        },
        "layers": [
            {"id": "imagery", "type": "raster", "source": "imagery"},
            {"id": "osm", "type": "raster", "source": "osm"},
            {"id": "roads", "type": "line", "source": "roads", "source-layer": "roads",
                "minzoom": 10},
            {"id": "hidden", "type": "raster", "source": "hidden",
                "layout": {"visibility": "none"}}
        ]
    }))
    .expect("style")
}

#[test]
fn credits_follow_the_layers_drawn_at_the_zoom_and_the_terrain() {
    let style = style();
    assert_eq!(
        style.attributions(5.0),
        ["USGS NAIP", "OpenStreetMap", "Mapzen Terrain Tiles"]
    );
    // From zoom 10 the roads draw; their credit contains the shorter OpenStreetMap one.
    assert_eq!(
        style.attributions(12.0),
        [
            "USGS NAIP",
            "© OpenStreetMap contributors",
            "Mapzen Terrain Tiles"
        ]
    );
}
