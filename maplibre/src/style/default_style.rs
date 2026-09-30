//! Built-in OpenMapTiles layer palette and initial view settings.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use csscolorparser::Color;

use super::{
    layer::{
        BackgroundPaint, FillPaint, LayerPaint, LinePaint, RasterPaint, StyleLayer, StyleProperty,
        SymbolPaint,
    },
    Style,
};

impl Default for Style {
    fn default() -> Self {
        Self {
            version: 8,
            glyphs: None,
            sprite: None,
            name: Some("Default Style".into()),
            metadata: Default::default(),
            sources: Default::default(),
            center: Some([50.85045, 4.34878]),
            bearing: Some(0.0),
            pitch: Some(0.0),
            roll: Some(0.0),
            vertical_field_of_view: None,
            projection: None,
            light: None,
            sky: None,
            terrain: None,
            state: Default::default(),
            global_state: Default::default(),
            state_templates: Default::default(),
            images: Default::default(),
            zoom: Some(13.0),
            layers: default_layers(),
        }
    }
}

fn default_layers() -> Vec<StyleLayer> {
    [
        layer(
            "background",
            "background",
            None,
            LayerPaint::Background(BackgroundPaint {
                background_color: Some(color([255, 255, 255])),
                background_opacity: None,
                background_pattern: None,
            }),
        ),
        fill("park", [200, 250, 204]),
        fill("landuse", [224, 223, 223]),
        fill("landcover", [174, 223, 163]),
        line("transportation", [255, 255, 255]),
        fill("building", [217, 208, 201]),
        fill("water", [170, 211, 223]),
        fill("waterway", [170, 211, 223]),
        line("boundary", [0, 0, 0]),
        layer(
            "raster",
            "raster",
            None,
            LayerPaint::Raster(RasterPaint::default()),
        ),
        symbol("text", "place"),
        symbol("transportation_name", "transportation_name-disabled"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, mut layer)| {
        layer.index = index as u32;
        layer
    })
    .collect()
}

fn layer(id: &str, kind: &str, source_layer: Option<&str>, paint: LayerPaint) -> StyleLayer {
    StyleLayer {
        id: id.into(),
        type_: kind.into(),
        source_layer: source_layer.map(str::to_owned),
        paint: Some(paint),
        ..Default::default()
    }
}

fn color([red, green, blue]: [u8; 3]) -> StyleProperty<Color> {
    StyleProperty::Constant(Color::from_rgba8(red, green, blue, 255))
}

fn fill(id: &str, rgb: [u8; 3]) -> StyleLayer {
    layer(
        id,
        "fill",
        Some(id),
        LayerPaint::Fill(FillPaint {
            fill_color: Some(color(rgb)),
            ..Default::default()
        }),
    )
}

fn line(id: &str, rgb: [u8; 3]) -> StyleLayer {
    layer(
        id,
        "line",
        Some(id),
        LayerPaint::Line(LinePaint {
            line_color: Some(color(rgb)),
            ..Default::default()
        }),
    )
}

fn symbol(id: &str, source_layer: &str) -> StyleLayer {
    layer(
        id,
        "symbol",
        Some(source_layer),
        LayerPaint::Symbol(SymbolPaint {
            text_field: Some(StyleProperty::parse(&serde_json::json!("{name}"))),
            ..Default::default()
        }),
    )
}
