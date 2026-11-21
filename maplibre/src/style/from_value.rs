//! Direct `&serde_json::Value` deserializer for [`Style`], eliminating serde derive bloat.

use rustc_hash::FxHashMap as HashMap;
use serde::de;
use serde_json::{Map, Value};

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
use super::light::LightSpecification;
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
use super::sky::SkySpecification;
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
use super::state::StateDeclaration;
#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
use super::transitions::TransitionSpec;
use super::{
    feature_state::FeatureStateStore, layer_order::parse_layers_from_values, source::Source,
    terrain::TerrainSpecification, Style, StyleImage,
};
use crate::{
    font::{default_local_ideograph_font_family, parse_local_ideograph_font_family, FontFaces},
    projection::ProjectionSpecification,
    util::json::extract::{
        field_err, json_bool_or, json_opt_f64, json_opt_f64_pair, json_opt_string,
        json_value_map_or_empty, missing_field_err, value_as_u64,
    },
};

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
impl StateDeclaration {
    /// Parses a `state.<key>` declaration directly from JSON.
    pub(crate) fn from_json_value(value: &Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(de::Error::custom("state declaration must be an object"));
        };
        Ok(Self {
            default: map.get("default").cloned(),
        })
    }
}

impl StyleImage {
    /// Parses a serialized [`StyleImage`] entry directly from JSON.
    pub(crate) fn from_json_value(value: &Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(de::Error::custom("style image must be an object"));
        };
        let width_val = map.get("width").ok_or_else(|| missing_field_err("width"))?;
        let width = u32::try_from(value_as_u64(width_val, "width")?)
            .map_err(|_| field_err("width", "u32 out of range"))?;
        let height_val = map
            .get("height")
            .ok_or_else(|| missing_field_err("height"))?;
        let height = u32::try_from(value_as_u64(height_val, "height")?)
            .map_err(|_| field_err("height", "u32 out of range"))?;
        let data_arr = map
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| missing_field_err("data"))?;
        let mut data = Vec::with_capacity(data_arr.len());
        for byte_val in data_arr {
            let b = u8::try_from(value_as_u64(byte_val, "data")?)
                .map_err(|_| field_err("data", "u8 out of range"))?;
            data.push(b);
        }
        let pixel_ratio = match map.get("pixel_ratio") {
            Some(v) => v
                .as_f64()
                .ok_or_else(|| field_err("pixel_ratio", "expected number"))?
                as f32,
            None => 1.0,
        };
        let sdf = json_bool_or(map, "sdf", false)?;
        Ok(Self {
            width,
            height,
            data,
            pixel_ratio,
            sdf,
        })
    }
}

fn parse_sources(map: &Map<String, Value>) -> Result<HashMap<String, Source>, serde_json::Error> {
    let Some(val) = map.get("sources") else {
        return Ok(HashMap::default());
    };
    if val.is_null() {
        return Ok(HashMap::default());
    }
    let Some(obj) = val.as_object() else {
        return Err(field_err("sources", "expected object"));
    };
    let mut out = HashMap::with_capacity_and_hasher(obj.len(), Default::default());
    for (name, entry) in obj {
        out.insert(name.clone(), Source::from_json_value(entry)?);
    }
    Ok(out)
}

fn parse_opt_section<T>(
    map: &Map<String, Value>,
    key: &str,
    parse: fn(&Value) -> Result<T, serde_json::Error>,
) -> Result<Option<T>, serde_json::Error> {
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(None);
    }
    parse(val).map(Some)
}

fn parse_style_images(
    map: &Map<String, Value>,
) -> Result<HashMap<String, StyleImage>, serde_json::Error> {
    let Some(val) = map.get("images") else {
        return Ok(HashMap::default());
    };
    if val.is_null() {
        return Ok(HashMap::default());
    }
    let Some(obj) = val.as_object() else {
        return Err(field_err("images", "expected object"));
    };
    let mut out = HashMap::with_capacity_and_hasher(obj.len(), Default::default());
    for (name, entry) in obj {
        out.insert(name.clone(), StyleImage::from_json_value(entry)?);
    }
    Ok(out)
}

#[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
fn parse_state_decl_map(
    map: &Map<String, Value>,
) -> Result<HashMap<String, StateDeclaration>, serde_json::Error> {
    let Some(val) = map.get("state") else {
        return Ok(HashMap::default());
    };
    if val.is_null() {
        return Ok(HashMap::default());
    }
    let Some(obj) = val.as_object() else {
        return Err(field_err("state", "expected object"));
    };
    let mut out = HashMap::with_capacity_and_hasher(obj.len(), Default::default());
    for (name, entry) in obj {
        out.insert(name.clone(), StateDeclaration::from_json_value(entry)?);
    }
    Ok(out)
}

impl Style {
    /// Parses a [`Style`] from UTF-8 JSON bytes via the shared [`Value`] parser.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_slice(bytes)?;
        Self::from_json_value(&value)
    }

    /// Parses a [`Style`] from a JSON string.
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        Self::from_json_slice(json.as_bytes())
    }

    /// Parses a [`Style`] directly from a [`serde_json::Value`] without per-field serde derives.
    pub fn from_json_value(value: &Value) -> Result<Self, serde_json::Error> {
        let Some(map) = value.as_object() else {
            return Err(de::Error::custom("style root must be a JSON object"));
        };
        let version_val = map
            .get("version")
            .ok_or_else(|| missing_field_err("version"))?;
        let version = u16::try_from(value_as_u64(version_val, "version")?)
            .map_err(|_| field_err("version", "u16 out of range"))?;

        let layers_arr = map
            .get("layers")
            .ok_or_else(|| missing_field_err("layers"))?
            .as_array()
            .ok_or_else(|| field_err("layers", "expected array"))?;

        let font_faces = map
            .get("font-faces")
            .or_else(|| map.get("fontFaces"))
            .filter(|v| !v.is_null())
            .map(FontFaces::from_json_value);

        let local_ideograph_font_family = match map
            .get("localIdeographFontFamily")
            .or_else(|| map.get("local-ideograph-font-family"))
        {
            None => default_local_ideograph_font_family(),
            Some(v) => parse_local_ideograph_font_family(Some(v)).map_err(de::Error::custom)?,
        };

        let feature_state = match map.get("featureState") {
            Some(v) if !v.is_null() => FeatureStateStore::from_json_value(v)?,
            _ => FeatureStateStore::default(),
        };

        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        let light = parse_opt_section(map, "light", LightSpecification::from_json_value)?;
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        let light = None;

        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        let sky = parse_opt_section(map, "sky", SkySpecification::from_json_value)?;
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        let sky = None;

        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        let transition = parse_opt_section(map, "transition", TransitionSpec::from_json_value)?;
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        let transition = None;

        let name = json_opt_string(map, "name")?;
        let glyphs = json_opt_string(map, "glyphs")?;
        let sprite = map.get("sprite").filter(|v| !v.is_null()).cloned();

        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        let metadata = json_value_map_or_empty(map, "metadata")?;
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        let metadata = HashMap::default();
        #[cfg(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl"))]
        let state = parse_state_decl_map(map)?;
        #[cfg(not(any(not(target_arch = "wasm32"), feature = "webgpu", feature = "web-webgl")))]
        let state: HashMap<String, StateDeclaration> = HashMap::default();
        let global_state = json_value_map_or_empty(map, "globalState")?;
        let (projection, projection_template) = match map.get("projection") {
            Some(raw) if !raw.is_null() && super::state::reads_global_state(raw) => {
                let resolved = super::state::resolve_projection_json(raw, &|key| {
                    global_state
                        .get(key)
                        .or_else(|| state.get(key)?.default.as_ref())
                        .cloned()
                        .unwrap_or(Value::Null)
                });
                (resolved, Some(raw.clone()))
            }
            _ => (
                parse_opt_section(map, "projection", ProjectionSpecification::from_json_value)?,
                None,
            ),
        };
        Ok(Self {
            version,
            name,
            metadata,
            sources: parse_sources(map)?,
            layers: parse_layers_from_values(layers_arr)?,
            glyphs,
            font_faces,
            local_ideograph_font_family,
            sprite,
            center: json_opt_f64_pair(map, "center")?,
            center_altitude: json_opt_f64(map, "centerAltitude")?,
            zoom: json_opt_f64(map, "zoom")?,
            bearing: json_opt_f64(map, "bearing")?,
            pitch: json_opt_f64(map, "pitch")?,
            roll: json_opt_f64(map, "roll")?,
            projection,
            light,
            sky,
            terrain: parse_opt_section(map, "terrain", TerrainSpecification::from_json_value)?,
            transition,
            active_transitions: Default::default(),
            state,
            global_state,
            feature_state,
            state_templates: HashMap::default(),
            light_template: None,
            projection_template,
            images: parse_style_images(map)?,
            revision_state: Default::default(),
        })
    }
}
