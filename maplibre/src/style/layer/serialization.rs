//! Layer JSON keeps unrecognized properties so validation can identify their original paths.

use serde::{de, ser::SerializeMap, Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{LayerPaint, LayerVisibility, StyleLayer, StyleProperty, SymbolPaint};
use crate::util::json::extract::{
    field_err, json_opt_f64, json_opt_string, json_opt_value_map, json_req_string,
};

/// JSON properties grouped by their paint or layout scope.
#[derive(Debug, Clone, Default)]
pub struct LayerProperties {
    /// Paint entries, including their values.
    pub paint: Map<String, Value>,
    /// Layout entries, including their values.
    pub layout: Map<String, Value>,
}

impl StyleLayer {
    /// Parses a [`StyleLayer`] directly from a [`serde_json::Value`] without per-field serde derives.
    pub fn from_json_value(value: &Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(de::Error::custom("layer must be a JSON object"));
        };
        let id = json_req_string(map, "id")?;
        let type_ = json_req_string(map, "type")?;
        let filter = map.get("filter").filter(|v| !v.is_null()).cloned();
        let maxzoom = json_opt_f64(map, "maxzoom")?;
        let minzoom = json_opt_f64(map, "minzoom")?;
        let metadata = json_opt_value_map(map, "metadata")?;
        let source = json_opt_string(map, "source")?;
        let source_layer = json_opt_string(map, "source-layer")?;
        let layout_map = match map.get("layout") {
            None | Some(Value::Null) => None,
            Some(Value::Object(obj)) => Some(obj),
            Some(_) => return Err(field_err("layout", "expected object")),
        };
        let paint_val = map.get("paint").filter(|v| !v.is_null());
        let rounded_from_layout = if type_ == "fill-extrusion" {
            layout_map.and_then(|l| l.get("fill-extrusion-rounded-corner-distance"))
        } else {
            None
        };
        let input_flags = Some(compute_input_flags(
            &type_,
            filter.as_ref(),
            paint_val,
            layout_map,
        ));
        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        let transitions = crate::style::transitions::extract_layer_transitions_ref(
            &id,
            paint_val.and_then(Value::as_object),
        )
        .map_err(de::Error::custom)?;
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        let transitions = Default::default();
        let mut unrecognized = LayerProperties::default();
        let mut paint = super::paint_from_value::parse_layer_paint_from_value(
            &type_,
            paint_val,
            rounded_from_layout,
            &mut unrecognized.paint,
        )
        .map_err(|error| de::Error::custom(format!("layer `{id}` paint: {error}")))?;
        let visibility = match layout_map
            .and_then(|l| l.get("visibility"))
            .filter(|v| !v.is_null())
        {
            Some(v) => super::paint_from_value::parse_visibility(v).map_err(|error| {
                de::Error::custom(format!("layer `{id}` layout.visibility: {error}"))
            })?,
            None => LayerVisibility::Visible,
        };
        let skip_rounded = type_ == "fill-extrusion";
        if let Some(LayerPaint::Symbol(symbol)) = &mut paint {
            merge_symbol_layout_ref(symbol, layout_map, &mut unrecognized);
        } else if let Some(layout) = layout_map {
            for (k, v) in layout {
                if k == "visibility"
                    || (skip_rounded && k == "fill-extrusion-rounded-corner-distance")
                {
                    continue;
                }
                unrecognized.layout.insert(k.clone(), v.clone());
            }
        }
        let _ = crate::style::StyleLayerId::intern(&id);
        if let Some(source_layer) = &source_layer {
            let _ = crate::style::SourceLayerId::intern(source_layer);
        }
        Ok(Self {
            index: 0,
            id,
            type_,
            filter,
            maxzoom,
            minzoom,
            metadata,
            paint,
            source,
            source_layer,
            visibility,
            transitions,
            unrecognized,
            input_flags,
        })
    }
}

impl<'de> Deserialize<'de> for StyleLayer {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_json_value(&value).map_err(de::Error::custom)
    }
}

fn json_reads_geometry(value: &Value) -> bool {
    match value {
        Value::Array(items) => {
            matches!(
                items.first().and_then(Value::as_str),
                Some("within" | "distance")
            ) || items.iter().any(json_reads_geometry)
        }
        Value::Object(map) => map.values().any(json_reads_geometry),
        _ => false,
    }
}

fn is_gpu_feature_state_paint_key(layer_type: &str, key: &str) -> bool {
    match layer_type {
        "fill" => matches!(key, "fill-color" | "fill-opacity" | "fill-outline-color"),
        "line" => matches!(key, "line-color" | "line-opacity"),
        "circle" => matches!(
            key,
            "circle-color" | "circle-opacity" | "circle-stroke-color"
        ),
        "fill-extrusion" => matches!(key, "fill-extrusion-color" | "fill-extrusion-opacity"),
        "symbol" => matches!(
            key,
            "text-color"
                | "text-halo-color"
                | "text-halo-width"
                | "text-halo-blur"
                | "text-opacity"
                | "icon-color"
                | "icon-halo-color"
                | "icon-halo-width"
                | "icon-halo-blur"
                | "icon-opacity"
        ),
        _ => false,
    }
}

fn compute_input_flags(
    layer_type: &str,
    filter: Option<&Value>,
    paint: Option<&Value>,
    layout: Option<&Map<String, Value>>,
) -> u8 {
    use super::{
        FLAG_GEOM_READS_FEATURE_STATE, FLAG_READS_FEATURE_STATE, FLAG_READS_GEOMETRY,
        FLAG_READS_IDENTITY,
    };
    use crate::style::{expression::reads_identity, feature_state::json_reads_feature_state};

    let mut flags = 0_u8;
    let sym_layout_reads = |pred: fn(&Value) -> bool| {
        layer_type == "symbol"
            && layout.is_some_and(|map| {
                map.iter().any(|(k, v)| {
                    k != "visibility" && crate::style::validation::symbol::supports(k) && pred(v)
                })
            })
    };
    if paint.is_some_and(reads_identity) || sym_layout_reads(reads_identity) {
        flags |= FLAG_READS_IDENTITY;
    }
    if filter.is_some_and(json_reads_geometry)
        || paint.is_some_and(json_reads_geometry)
        || layout.is_some_and(|map| map.values().any(json_reads_geometry))
    {
        flags |= FLAG_READS_GEOMETRY;
    }
    if paint.is_some_and(json_reads_feature_state) || sym_layout_reads(json_reads_feature_state) {
        flags |= FLAG_READS_FEATURE_STATE;
    }
    let geom_fs = filter.is_some_and(json_reads_feature_state)
        || layout.is_some_and(|map| {
            map.iter()
                .any(|(k, v)| k != "visibility" && json_reads_feature_state(v))
        })
        || paint.and_then(Value::as_object).is_some_and(|map| {
            map.iter().any(|(k, v)| {
                !is_gpu_feature_state_paint_key(layer_type, k) && json_reads_feature_state(v)
            })
        });
    if geom_fs {
        flags |= FLAG_GEOM_READS_FEATURE_STATE;
    }
    flags
}

fn merge_symbol_layout_ref(
    symbol: &mut SymbolPaint,
    layout: Option<&Map<String, Value>>,
    extra: &mut LayerProperties,
) {
    symbol.properties.retain(|name, value| {
        if crate::style::validation::symbol::supports(name) {
            true
        } else {
            extra.paint.insert(name.clone(), value.clone());
            false
        }
    });
    let Some(layout) = layout else {
        return;
    };
    for (name, value) in layout {
        match name.as_str() {
            "visibility" => {}
            "text-field" => symbol.text_field = Some(StyleProperty::parse(value)),
            "text-size" => symbol.text_size = Some(StyleProperty::parse(value)),
            _ if crate::style::validation::symbol::supports(name) => {
                symbol.properties.insert(name.clone(), value.clone());
            }
            _ => {
                extra.layout.insert(name.clone(), value.clone());
            }
        }
    }
}

impl Serialize for StyleLayer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("type", &self.type_)?;
        if let Some(value) = &self.filter {
            map.serialize_entry("filter", value)?;
        }
        if let Some(value) = &self.maxzoom {
            map.serialize_entry("maxzoom", value)?;
        }
        if let Some(value) = &self.minzoom {
            map.serialize_entry("minzoom", value)?;
        }
        if let Some(value) = &self.metadata {
            map.serialize_entry("metadata", value)?;
        }
        if let Some(value) = &self.source {
            map.serialize_entry("source", value)?;
        }
        if let Some(value) = &self.source_layer {
            map.serialize_entry("source-layer", value)?;
        }
        let properties = self.property_maps().map_err(serde::ser::Error::custom)?;
        let LayerProperties { paint, layout } = properties;
        if !paint.is_empty() || self.paint.is_some() {
            map.serialize_entry("paint", &paint)?;
        }
        if !layout.is_empty() {
            map.serialize_entry("layout", &layout)?;
        }
        map.end()
    }
}

impl StyleLayer {
    /// What GL JS `groupByLayout` compares: layers equal in type, source, source layer, zoom
    /// range, filter and every layout property share one symbol bucket and one placement.
    pub(crate) fn layout_group_key(&self) -> Option<String> {
        use std::hash::{Hash, Hasher};

        fn feed_json(hasher: &mut rustc_hash::FxHasher, value: &Value) {
            match value {
                Value::Null => 0_u8.hash(hasher),
                Value::Bool(b) => {
                    1_u8.hash(hasher);
                    b.hash(hasher);
                }
                Value::Number(n) => {
                    2_u8.hash(hasher);
                    if let Some(u) = n.as_u64() {
                        (0_u8, u).hash(hasher);
                    } else if let Some(i) = n.as_i64() {
                        (1_u8, i).hash(hasher);
                    } else if let Some(f) = n.as_f64() {
                        (2_u8, f.to_bits()).hash(hasher);
                    }
                }
                Value::String(s) => {
                    3_u8.hash(hasher);
                    s.hash(hasher);
                }
                Value::Array(items) => {
                    4_u8.hash(hasher);
                    items.len().hash(hasher);
                    for item in items {
                        feed_json(hasher, item);
                    }
                }
                Value::Object(entries) => {
                    5_u8.hash(hasher);
                    entries.len().hash(hasher);
                    for (k, v) in entries {
                        k.hash(hasher);
                        feed_json(hasher, v);
                    }
                }
            }
        }

        fn feed_style_prop<T, F: Fn(&T, &mut rustc_hash::FxHasher)>(
            hasher: &mut rustc_hash::FxHasher,
            prop: &StyleProperty<T>,
            hash_const: F,
        ) {
            match prop {
                StyleProperty::Constant(v) => {
                    0_u8.hash(hasher);
                    hash_const(v, hasher);
                }
                StyleProperty::Expression(expr) => {
                    1_u8.hash(hasher);
                    feed_json(hasher, expr.source());
                }
                StyleProperty::Unsupported(raw) => {
                    2_u8.hash(hasher);
                    feed_json(hasher, &raw.source);
                }
            }
        }

        let mut h0 = rustc_hash::FxHasher::default();
        let mut h1 = rustc_hash::FxHasher::default();
        0x243f_6a88_85a3_08d3_u64.hash(&mut h0);
        0x1319_8a2e_0370_7344_u64.hash(&mut h1);

        for hasher in [&mut h0, &mut h1] {
            self.type_.hash(hasher);
            self.source.hash(hasher);
            self.source_layer.hash(hasher);
            self.minzoom.map(f64::to_bits).hash(hasher);
            self.maxzoom.map(f64::to_bits).hash(hasher);
            if let Some(filter) = &self.filter {
                1_u8.hash(hasher);
                feed_json(hasher, filter);
            } else {
                0_u8.hash(hasher);
            }
            for (k, v) in &self.unrecognized.layout {
                if matches!(k.as_str(), "text-field" | "text-size" | "visibility") {
                    continue;
                }
                if let Some(LayerPaint::Symbol(sym)) = &self.paint {
                    if sym.properties.contains_key(k) {
                        continue;
                    }
                }
                k.hash(hasher);
                feed_json(hasher, v);
            }
            if let Some(LayerPaint::Symbol(symbol)) = &self.paint {
                if let Some(field) = &symbol.text_field {
                    "text-field".hash(hasher);
                    feed_style_prop(hasher, field, |ft, h| ft.hash_layout_key(h));
                }
                if let Some(size) = &symbol.text_size {
                    "text-size".hash(hasher);
                    feed_style_prop(hasher, size, |s, h| s.to_bits().hash(h));
                }
                for (name, value) in &symbol.properties {
                    if crate::style::validation::symbol::is_layout(name) {
                        name.hash(hasher);
                        feed_json(hasher, value);
                    }
                }
            }
            if self.is_hidden() {
                "visibility".hash(hasher);
                "none".hash(hasher);
            }
        }
        Some(format!("{:016x}{:016x}", h0.finish(), h1.finish()))
    }

    pub(crate) fn property_maps(&self) -> Result<LayerProperties, serde_json::Error> {
        let mut paint = self.unrecognized.paint.clone();
        let mut layout = self.unrecognized.layout.clone();
        if let Some(model) = &self.paint {
            let tagged = serde_json::to_value(model)?;
            if let Some(properties) = tagged.get("paint").and_then(Value::as_object) {
                for (name, value) in properties {
                    let destination = if matches!(model, LayerPaint::Symbol(_))
                        && crate::style::validation::symbol::is_layout(name)
                    {
                        &mut layout
                    } else {
                        &mut paint
                    };
                    destination.insert(name.clone(), value.clone());
                }
            }
        }
        for (property, spec) in &self.transitions {
            paint.insert(
                format!("{property}-transition"),
                serde_json::to_value(spec)?,
            );
        }
        if self.is_hidden() {
            layout.insert("visibility".into(), Value::String("none".into()));
        }
        Ok(LayerProperties { paint, layout })
    }
}
