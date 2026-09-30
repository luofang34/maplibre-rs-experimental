//! Sprite images that layers repeat as patterns, added to the style as images.

use std::{collections::BTreeSet, path::PathBuf};

use maplibre::style::{layer::LayerPaint, Style, StyleImage};
use serde_json::Value;

/// Every string an expression or a plain value mentions; the names of images among them.
fn strings(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            found.insert(text.clone());
        }
        Value::Array(items) => items.iter().for_each(|item| strings(item, found)),
        Value::Object(map) => map.values().for_each(|item| strings(item, found)),
        _ => {}
    }
}

fn sprite_base(style: &Style) -> Option<PathBuf> {
    let url = style.sprite.as_ref()?.as_str()?;
    let relative = url.strip_prefix("local://sprites/")?;
    Some(PathBuf::from("render-tests/src/assets/sprites").join(relative))
}

fn with_extension(base: &std::path::Path, suffix: &str, extension: &str) -> PathBuf {
    let mut name = base.as_os_str().to_owned();
    name.push(format!("{suffix}.{extension}"));
    PathBuf::from(name)
}

/// Adds the sprite images that pattern properties name, at the sprite sheet for the pixel
/// ratio when the fixture ships one.
pub(super) fn add_pattern_images(style: &mut Style, pixel_ratio: f64) -> Result<(), String> {
    let mut wanted = BTreeSet::new();
    for layer in &style.layers {
        let pattern = match &layer.paint {
            Some(LayerPaint::Fill(paint)) => paint.fill_pattern.as_ref(),
            Some(LayerPaint::Line(paint)) => paint.line_pattern.as_ref(),
            Some(LayerPaint::FillExtrusion(paint)) => paint.fill_extrusion_pattern.as_ref(),
            _ => None,
        };
        if let Some(pattern) = pattern {
            strings(pattern, &mut wanted);
        }
    }
    let Some(base) = sprite_base(style).filter(|_| !wanted.is_empty()) else {
        return Ok(());
    };
    let retina = pixel_ratio > 1.0 && with_extension(&base, "@2x", "json").exists();
    let suffix = if retina { "@2x" } else { "" };
    let json_path = with_extension(&base, suffix, "json");
    let Ok(text) = std::fs::read_to_string(&json_path) else {
        return Ok(());
    };
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("Cannot parse {}: {error}", json_path.display()))?;
    let png_path = with_extension(&base, suffix, "png");
    let sheet = image::open(&png_path)
        .map_err(|error| format!("Cannot read {}: {error}", png_path.display()))?
        .to_rgba8();
    for name in wanted {
        let Some(entry) = document.get(&name) else {
            continue;
        };
        let read = |key: &str| entry.get(key).and_then(Value::as_u64).map(|v| v as u32);
        let (Some(x), Some(y), Some(width), Some(height)) =
            (read("x"), read("y"), read("width"), read("height"))
        else {
            continue;
        };
        if x + width > sheet.width() || y + height > sheet.height() {
            continue;
        }
        let pixels = image::imageops::crop_imm(&sheet, x, y, width, height).to_image();
        style.images.insert(
            name,
            StyleImage {
                width,
                height,
                data: pixels.into_raw(),
                pixel_ratio: entry
                    .get("pixelRatio")
                    .and_then(Value::as_f64)
                    .map_or(1.0, |ratio| ratio as f32),
                sdf: entry.get("sdf").and_then(Value::as_bool).unwrap_or(false),
            },
        );
    }
    Ok(())
}
