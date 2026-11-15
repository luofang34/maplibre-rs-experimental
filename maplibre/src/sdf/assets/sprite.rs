//! Compact typed sprite-sheet metadata parsed once when a sprite JSON document loads.

use std::collections::HashMap;

use super::{IconStretch, TextFit};

/// Pre-parsed atlas coordinates and stretch layout of a single sprite icon.
pub(super) struct SpriteEntry {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixel_ratio: f32,
    pub(super) sdf: bool,
    pub(super) stretch: Option<Box<IconStretch>>,
}

pub(super) fn parse_sprite_document(
    bytes: &[u8],
) -> Result<HashMap<String, SpriteEntry>, serde_json::Error> {
    let raw: HashMap<String, serde_json::Value> = serde_json::from_slice(bytes)?;
    let mut out = HashMap::with_capacity(raw.len());
    for (name, value) in raw {
        let read = |key: &str| {
            value
                .get(key)
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
        };
        let (Some(x), Some(y), Some(width), Some(height)) =
            (read("x"), read("y"), read("width"), read("height"))
        else {
            continue;
        };
        let sdf = value.get("sdf").and_then(|v| v.as_bool()).unwrap_or(false);
        let pixel_ratio = value
            .get("pixelRatio")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0)
            .max(0.01) as f32;
        out.insert(
            name,
            SpriteEntry {
                x,
                y,
                width,
                height,
                pixel_ratio,
                sdf,
                stretch: icon_stretch(&value),
            },
        );
    }
    Ok(out)
}

fn ranges(value: &serde_json::Value, key: &str) -> Vec<[f32; 2]> {
    value
        .get(key)
        .and_then(|ranges| ranges.as_array())
        .into_iter()
        .flatten()
        .filter_map(|range| {
            let range = range.as_array()?;
            Some([
                range.first()?.as_f64()? as f32,
                range.get(1)?.as_f64()? as f32,
            ])
        })
        .collect()
}

fn text_fit(value: &serde_json::Value, key: &str) -> Option<TextFit> {
    match value.get(key)?.as_str()? {
        "stretchOrShrink" => Some(TextFit::StretchOrShrink),
        "stretchOnly" => Some(TextFit::StretchOnly),
        "proportional" => Some(TextFit::Proportional),
        _ => None,
    }
}

fn icon_stretch(value: &serde_json::Value) -> Option<Box<IconStretch>> {
    let stretch = IconStretch {
        stretch_x: ranges(value, "stretchX"),
        stretch_y: ranges(value, "stretchY"),
        content: value.get("content").and_then(|content| {
            let content = content.as_array()?;
            let read = |index: usize| content.get(index)?.as_f64().map(|edge| edge as f32);
            Some([read(0)?, read(1)?, read(2)?, read(3)?])
        }),
        text_fit_width: text_fit(value, "textFitWidth"),
        text_fit_height: text_fit(value, "textFitHeight"),
    };
    (stretch != IconStretch::default()).then(|| Box::new(stretch))
}
