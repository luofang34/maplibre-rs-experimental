#![allow(clippy::expect_used, clippy::panic)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use maplibre::style::{layer::StyleLayer, Style};
use serde_json::Value;

fn collect_style_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("readable render-tests directory");
    for entry in entries {
        let path = entry.expect("readable directory entry").path();
        if path.is_dir() {
            collect_style_files(&path, out);
        } else if path.file_name().is_some_and(|name| name == "style.json") {
            out.push(path);
        }
    }
}

fn check_no_null_fields_in_paint_or_light(style_json: &Value, context: &str) -> Result<(), String> {
    if let Some(light) = style_json.get("light").and_then(Value::as_object) {
        for (k, v) in light {
            if v.is_null() {
                return Err(format!("{context}: unexpected null light.{k}"));
            }
        }
    }
    if let Some(layers) = style_json.get("layers").and_then(Value::as_array) {
        for layer in layers {
            let id = layer.get("id").and_then(Value::as_str).unwrap_or("?");
            if let Some(paint) = layer.get("paint").and_then(Value::as_object) {
                for (k, v) in paint {
                    if v.is_null() {
                        return Err(format!(
                            "{context}: unexpected null paint.{k} on layer `{id}`"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn assert_layers_structurally_equal(
    a: &StyleLayer,
    b: &StyleLayer,
    context: &str,
) -> Result<(), String> {
    if a.id != b.id
        || a.type_ != b.type_
        || a.source != b.source
        || a.source_layer != b.source_layer
        || a.visibility != b.visibility
        || a.minzoom != b.minzoom
        || a.maxzoom != b.maxzoom
        || a.filter != b.filter
        || a.metadata != b.metadata
        || a.transitions != b.transitions
        || a.unrecognized != b.unrecognized
    {
        return Err(format!(
            "{context}: layer `{}` structural metadata mismatch",
            a.id
        ));
    }
    let pa = serde_json::to_value(&a.paint).map_err(|e| format!("{context}: {e}"))?;
    let pb = serde_json::to_value(&b.paint).map_err(|e| format!("{context}: {e}"))?;
    if pa != pb {
        return Err(format!(
            "{context}: layer `{}` paint mismatch:\nleft: {pa}\nright: {pb}",
            a.id
        ));
    }
    Ok(())
}

#[test]
fn round_trip_all_render_test_styles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../render-tests/src/tests");
    let mut style_paths = Vec::new();
    collect_style_files(&root, &mut style_paths);
    style_paths.sort();

    let mut checked = 0usize;
    let mut failures = Vec::new();

    for path in &style_paths {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("projection/perspective/") {
            continue;
        }
        checked += 1;

        let raw = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) => {
                failures.push(format!("{rel}: read error: {err}"));
                continue;
            }
        };
        let s1: Style = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(err) => {
                failures.push(format!("{rel}: initial parse error: {err}"));
                continue;
            }
        };
        let v1 = match serde_json::to_value(&s1) {
            Ok(v) => v,
            Err(err) => {
                failures.push(format!("{rel}: initial serialize error: {err}"));
                continue;
            }
        };
        if let Err(err) = check_no_null_fields_in_paint_or_light(&v1, &rel) {
            failures.push(err);
            continue;
        }
        let s2: Style = match serde_json::from_value(v1.clone()) {
            Ok(s) => s,
            Err(err) => {
                failures.push(format!("{rel}: re-parse error: {err}\njson: {v1}"));
                continue;
            }
        };
        let v2 = match serde_json::to_value(&s2) {
            Ok(v) => v,
            Err(err) => {
                failures.push(format!("{rel}: second serialize error: {err}"));
                continue;
            }
        };
        if v1 != v2 {
            failures.push(format!("{rel}: non-idempotent serialization (v1 != v2)"));
            continue;
        }
        if s1.version != s2.version
            || s1.sources.len() != s2.sources.len()
            || s1.layers.len() != s2.layers.len()
            || s1.state != s2.state
            || s1.font_faces != s2.font_faces
        {
            failures.push(format!("{rel}: root style mismatch after round-trip"));
            continue;
        }
        for (k, src1) in &s1.sources {
            let Some(src2) = s2.sources.get(k) else {
                failures.push(format!("{rel}: missing source `{k}` after round-trip"));
                break;
            };
            if std::mem::discriminant(src1) != std::mem::discriminant(src2) {
                failures.push(format!("{rel}: source `{k}` kind changed after round-trip"));
                break;
            }
        }
        for (l1, l2) in s1.layers.iter().zip(s2.layers.iter()) {
            if let Err(err) = assert_layers_structurally_equal(l1, l2, &rel) {
                failures.push(err);
                break;
            }
        }
    }

    assert!(
        checked >= 1700,
        "expected >= 1700 active render-test styles, checked only {checked}"
    );
    assert!(
        failures.is_empty(),
        "{} / {checked} style round-trip failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn round_trip_font_face_unicode_ranges() {
    let raw = serde_json::json!({
        "version": 8,
        "font-faces": {
            "Noto Sans": [{
                "url": "https://example.invalid/noto.woff2",
                "unicode-range": "U+0000-00FF, U+4E00-9FFF, U+3042"
            }]
        },
        "layers": []
    });
    let s1: Style = serde_json::from_value(raw).expect("parse style with font-faces");
    let v1 = serde_json::to_value(&s1).expect("serialize style");
    let s2: Style = serde_json::from_value(v1.clone()).expect("re-parse style");
    let v2 = serde_json::to_value(&s2).expect("re-serialize style");

    assert_eq!(v1, v2);
    assert_eq!(s1.font_faces, s2.font_faces);
    let faces = s2.font_faces.expect("font-faces preserved");
    let face = &faces.0[0];
    assert_eq!(face.family, "Noto Sans");
    assert_eq!(face.unicode_ranges.len(), 3);
    assert_eq!(
        (face.unicode_ranges[0].start, face.unicode_ranges[0].end),
        (0x0000, 0x00FF)
    );
    assert_eq!(
        (face.unicode_ranges[1].start, face.unicode_ranges[1].end),
        (0x4E00, 0x9FFF)
    );
    assert_eq!(
        (face.unicode_ranges[2].start, face.unicode_ranges[2].end),
        (0x3042, 0x3042)
    );
}

#[test]
fn round_trip_formatted_text_literal_braces() {
    let raw = serde_json::json!({
        "version": 8,
        "sources": {
            "pts": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "layers": [{
            "id": "labels",
            "type": "symbol",
            "source": "pts",
            "layout": {
                "text-field": ["concat", "{", "name", "}"]
            }
        }]
    });
    let s1: Style = serde_json::from_value(raw).expect("parse literal braces text-field");
    let v1 = serde_json::to_value(&s1).expect("serialize style");
    let s2: Style = serde_json::from_value(v1.clone()).expect("re-parse style");
    let v2 = serde_json::to_value(&s2).expect("re-serialize style");
    assert_eq!(v1, v2);

    let maplibre::style::layer::LayerPaint::Symbol(sym) =
        s2.layers[0].paint.as_ref().expect("symbol paint")
    else {
        panic!("expected symbol paint");
    };
    let field = sym.text_field.as_ref().expect("text-field");
    assert!(
        field.is_feature_constant(),
        "literal '{{name}}' must stay constant across round-trip, got {v1}"
    );
    let evaluated = field
        .evaluate_at_zoom(0.0)
        .expect("constant evaluates without feature");
    assert_eq!(evaluated.0 .0, "{name}");
}

#[test]
fn round_trip_no_null_optional_paint_and_light_fields() {
    let raw = serde_json::json!({
        "version": 8,
        "light": { "anchor": "viewport", "position": [1.15, 210, 30] },
        "sources": {
            "s": { "type": "geojson", "data": { "type": "FeatureCollection", "features": [] } }
        },
        "layers": [
            { "id": "f", "type": "fill", "source": "s", "paint": { "fill-color": "#ff0000" } },
            { "id": "l", "type": "line", "source": "s", "paint": { "line-color": "#00ff00" } },
            { "id": "c", "type": "circle", "source": "s", "paint": { "circle-color": "#0000ff" } },
            { "id": "e", "type": "fill-extrusion", "source": "s", "paint": { "fill-extrusion-color": "#ffffff" } }
        ]
    });
    let s1: Style = serde_json::from_value(raw).expect("parse minimal paint/light style");
    let v1 = serde_json::to_value(&s1).expect("serialize");
    check_no_null_fields_in_paint_or_light(&v1, "minimal").expect("no null fields");
    let s2: Style = serde_json::from_value(v1.clone()).expect("re-parse");
    let v2 = serde_json::to_value(&s2).expect("re-serialize");
    assert_eq!(v1, v2);
}
