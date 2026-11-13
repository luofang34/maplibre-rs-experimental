//! Validates shaders, generates protobuf and style-spec v8 bindings, and embeds static tiles.
use std::{
    env,
    fmt::Write as _,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use maplibre_build_tools::wgsl::validate_project_wgsl_blocking;
use serde_json::{Map, Value};

type BuildResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[cfg(feature = "embed-static-tiles")]
fn embed_tiles_statically_blocking() -> BuildResult {
    use maplibre_build_tools::mbtiles::extract_blocking;

    const MUNICH_X: u32 = 17425;
    const MUNICH_Y: u32 = 11365;
    const MUNICH_Z: u8 = 15;
    let out = Path::new(&env::var("OUT_DIR")?).join("extracted-tiles");
    if out.is_dir() {
        fs::remove_dir_all(&out)?;
    }
    let source = Path::new(&env::var("CARGO_MANIFEST_DIR")?)
        .join(format!("../test-data/munich-{MUNICH_Z}.mbtiles"));
    if source.exists() {
        writeln!(io::stdout().lock(), "cargo:rustc-cfg=static_tiles_found")?;
        extract_blocking(
            source,
            out,
            MUNICH_Z,
            (MUNICH_X - 2)..(MUNICH_X + 2),
            (MUNICH_Y - 2)..(MUNICH_Y + 2),
        )?;
    }
    Ok(())
}

fn generate_protobuf_blocking() -> BuildResult {
    let mut paths = Vec::new();
    for entry in fs::read_dir("./proto")? {
        let path = entry?.path();
        writeln!(
            io::stdout().lock(),
            "cargo:rerun-if-changed={}",
            path.display()
        )?;
        paths.push(path);
    }
    prost_build::compile_protos(&paths, &[PathBuf::from("./proto/")])?;
    Ok(())
}

fn to_pascal_case(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn to_screaming_snake_case(kebab: &str) -> String {
    kebab
        .chars()
        .map(|ch| {
            if ch == '-' {
                '_'
            } else {
                ch.to_ascii_uppercase()
            }
        })
        .collect()
}

fn map_param_variant(param: &str) -> BuildResult<&'static str> {
    match param {
        "zoom" => Ok("SpecExpressionParameter::Zoom"),
        "feature" => Ok("SpecExpressionParameter::Feature"),
        "feature-state" => Ok("SpecExpressionParameter::FeatureState"),
        "line-progress" => Ok("SpecExpressionParameter::LineProgress"),
        "heatmap-density" => Ok("SpecExpressionParameter::HeatmapDensity"),
        "sky-radial-progress" => Ok("SpecExpressionParameter::SkyRadialProgress"),
        "measure-light" => Ok("SpecExpressionParameter::MeasureLight"),
        "elevation" => Ok("SpecExpressionParameter::Elevation"),
        other => Err(format!("unknown style-spec expression parameter `{other}`").into()),
    }
}

fn map_value_type_expr(prop: &Map<String, Value>) -> BuildResult<String> {
    let raw_type = prop
        .get("type")
        .and_then(Value::as_str)
        .ok_or("style-spec property entry missing string `type`")?;
    match raw_type {
        "number" => Ok("SpecValueType::Number".to_owned()),
        "color" => Ok("SpecValueType::Color".to_owned()),
        "string" => Ok("SpecValueType::String".to_owned()),
        "boolean" => Ok("SpecValueType::Boolean".to_owned()),
        "enum" => Ok("SpecValueType::Enum".to_owned()),
        "formatted" => Ok("SpecValueType::Formatted".to_owned()),
        "resolvedImage" => Ok("SpecValueType::ResolvedImage".to_owned()),
        "padding" => Ok("SpecValueType::Padding".to_owned()),
        "numberArray" => Ok("SpecValueType::NumberArray".to_owned()),
        "colorArray" => Ok("SpecValueType::ColorArray".to_owned()),
        "variableAnchorOffsetCollection" => {
            Ok("SpecValueType::VariableAnchorOffsetCollection".to_owned())
        }
        "projectionDefinition" => Ok("SpecValueType::ProjectionDefinition".to_owned()),
        "array" => {
            let item = match prop.get("value").and_then(Value::as_str) {
                Some("number") => "SpecArrayItemType::Number",
                Some("string") => "SpecArrayItemType::String",
                Some("boolean") => "SpecArrayItemType::Boolean",
                Some("enum") => "SpecArrayItemType::Enum",
                Some("value") | None => "SpecArrayItemType::Value",
                Some(other) => {
                    return Err(format!("unknown style-spec array item type `{other}`").into());
                }
            };
            let length = prop
                .get("length")
                .and_then(Value::as_u64)
                .map_or_else(|| "None".to_owned(), |len| format!("Some({len})"));
            Ok(format!(
                "SpecValueType::Array {{ item: {item}, length: {length} }}"
            ))
        }
        other => Err(format!("unknown style-spec value type `{other}`").into()),
    }
}

fn emit_property_meta(
    out: &mut String,
    layer_variant: &str,
    kind_variant: &str,
    prop_name: &str,
    prop: &Map<String, Value>,
) -> BuildResult {
    let value_type = map_value_type_expr(prop)?;
    let (supports_interpolated, supports_piecewise_constant, params_expr) = if let Some(expr) =
        prop.get("expression").and_then(Value::as_object)
    {
        let raw = expr
            .get("parameters")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let mapped = raw
            .iter()
            .map(|p| {
                p.as_str()
                    .ok_or("param must be string".into())
                    .and_then(map_param_variant)
            })
            .collect::<BuildResult<Vec<_>>>()?;
        let has_params = !mapped.is_empty();
        let interp = has_params && expr.get("interpolated").and_then(Value::as_bool) == Some(true);
        (interp, has_params, format!("&[{}]", mapped.join(", ")))
    } else {
        (false, false, "&[]".to_owned())
    };

    let enum_values_expr = match prop.get("values") {
        Some(Value::Object(map)) => {
            let quoted: Vec<String> = map.keys().map(|k| format!("{k:?}")).collect();
            format!("&[{}]", quoted.join(", "))
        }
        Some(Value::Array(arr)) => {
            let quoted = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| format!("{s:?}"))
                        .ok_or_else(|| "enum entry must be string".into())
                })
                .collect::<BuildResult<Vec<_>>>()?;
            format!("&[{}]", quoted.join(", "))
        }
        _ => "&[]".to_owned(),
    };

    let supports_transition = prop.get("transition").and_then(Value::as_bool) == Some(true);
    let supports_tokens = prop.get("tokens").and_then(Value::as_bool) == Some(true);
    let default_json_expr = match prop.get("default") {
        Some(val) => format!("Some({:?})", serde_json::to_string(val)?),
        None => "None".to_owned(),
    };
    let doc = prop.get("doc").and_then(Value::as_str).unwrap_or("");

    writeln!(
        out,
        "    SpecPropertyMeta {{ name: {prop_name:?}, layer_type: SpecLayerType::{layer_variant}, \
         kind: SpecPropertyKind::{kind_variant}, value_type: {value_type}, \
         expression: SpecPropertyExpression {{ supports_interpolated: {supports_interpolated}, \
         supports_piecewise_constant: {supports_piecewise_constant}, parameters: {params_expr} }}, \
         enum_values: {enum_values_expr}, supports_transition: {supports_transition}, \
         supports_tokens: {supports_tokens}, default_json: {default_json_expr}, doc: {doc:?} }},"
    )?;
    Ok(())
}

fn emit_property_table_and_enum(
    out: &mut String,
    layer_name: &str,
    layer_variant: &str,
    kind_label: &str,
    kind_variant: &str,
    table: &Map<String, Value>,
) -> BuildResult {
    let const_name = format!(
        "{}_{}_PROPERTIES",
        to_screaming_snake_case(layer_name),
        kind_label.to_uppercase()
    );
    let enum_name = format!("{layer_variant}{kind_variant}Property");

    writeln!(out, "static {const_name}: &[SpecPropertyMeta] = &[")?;
    for (prop_name, prop_val) in table {
        let prop_obj = prop_val
            .as_object()
            .ok_or_else(|| format!("property `{prop_name}` in `{layer_name}` is not an object"))?;
        emit_property_meta(out, layer_variant, kind_variant, prop_name, prop_obj)?;
    }
    writeln!(out, "];\n")?;

    writeln!(
        out,
        "/// Typed `{kind_label}` property names for a `{layer_name}` style layer.\n\
         #[allow(clippy::enum_variant_names)]\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n\
         pub enum {enum_name} {{"
    )?;
    let mut variants = Vec::with_capacity(table.len());
    for prop_name in table.keys() {
        let variant = to_pascal_case(prop_name);
        writeln!(
            out,
            "    /// The `{prop_name}` {kind_label} property.\n    {variant},"
        )?;
        variants.push((prop_name.as_str(), variant));
    }
    let all_list: Vec<String> = variants.iter().map(|(_, v)| format!("Self::{v}")).collect();
    writeln!(
        out,
        "}}\n\nimpl {enum_name} {{\n    /// Every `{kind_label}` property defined for `{layer_name}` layers.\n\
         \x20   pub const ALL: &'static [Self] = &[{}];\n\
         \x20   /// Canonical kebab-case property name in the MapLibre Style Spec.\n\
         \x20   pub const fn as_str(self) -> &'static str {{ match self {{",
        all_list.join(", ")
    )?;
    for (prop_name, variant) in &variants {
        writeln!(out, "        Self::{variant} => {prop_name:?},")?;
    }
    writeln!(
        out,
        "    }} }}\n    /// Static style-spec descriptor for this property.\n\
         \x20   pub fn metadata(self) -> &'static SpecPropertyMeta {{ match self {{"
    )?;
    for (idx, (_, variant)) in variants.iter().enumerate() {
        writeln!(out, "        Self::{variant} => &{const_name}[{idx}],")?;
    }
    writeln!(
        out,
        "    }} }}\n}}\n\nimpl std::fmt::Display for {enum_name} {{\n\
         \x20   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ f.write_str(self.as_str()) }}\n\
         }}\n\nimpl std::str::FromStr for {enum_name} {{\n    type Err = UnknownSpecIdentifier;\n\
         \x20   fn from_str(s: &str) -> Result<Self, Self::Err> {{ match s {{"
    )?;
    for (prop_name, variant) in &variants {
        writeln!(out, "        {prop_name:?} => Ok(Self::{variant}),")?;
    }
    writeln!(
        out,
        "        _ => Err(UnknownSpecIdentifier(s.to_owned())),\n    }} }}\n}}\n"
    )?;
    Ok(())
}

fn generate_style_spec_blocking() -> BuildResult {
    writeln!(
        io::stdout().lock(),
        "cargo:rerun-if-changed=style-spec-v8.json"
    )?;
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root_val: Value = serde_json::from_str(&fs::read_to_string(
        manifest_dir.join("style-spec-v8.json"),
    )?)?;
    let root = root_val
        .as_object()
        .ok_or("style-spec-v8.json root must be an object")?;
    let v8_layer_values = root
        .get("layer")
        .and_then(|l| l.get("type"))
        .and_then(|t| t.get("values"))
        .and_then(Value::as_object)
        .ok_or("missing `layer.type.values` in style-spec-v8.json")?;

    let v8_layers: Vec<(String, String)> = v8_layer_values
        .keys()
        .map(|name| (name.clone(), to_pascal_case(name)))
        .collect();
    let mut all_layers = v8_layers.clone();
    for ext in ["color-relief", "custom"] {
        if !all_layers.iter().any(|(name, _)| name == ext) {
            all_layers.push((ext.to_owned(), to_pascal_case(ext)));
        }
    }

    let mut code = String::with_capacity(64 * 1024);
    writeln!(
        code,
        "/// Style-spec layer `type` discriminator generated from `style-spec-v8.json`.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n\
         pub enum SpecLayerType {{"
    )?;
    for (name, variant) in &all_layers {
        writeln!(
            code,
            "    /// The `{name}` style layer type.\n    {variant},"
        )?;
    }
    let all_variants: Vec<String> = all_layers
        .iter()
        .map(|(_, v)| format!("Self::{v}"))
        .collect();
    writeln!(
        code,
        "}}\n\nimpl SpecLayerType {{\n    /// Every recognized style layer type.\n\
         \x20   pub const ALL: &'static [Self] = &[{}];\n\
         \x20   /// Canonical kebab-case layer type string.\n\
         \x20   pub const fn as_str(self) -> &'static str {{ match self {{",
        all_variants.join(", ")
    )?;
    for (name, variant) in &all_layers {
        writeln!(code, "        Self::{variant} => {name:?},")?;
    }
    writeln!(
        code,
        "    }} }}\n    /// Paint property metadata slice for this layer type.\n\
         \x20   pub fn paint_properties(self) -> &'static [SpecPropertyMeta] {{ match self {{"
    )?;
    for (name, variant) in &v8_layers {
        writeln!(
            code,
            "        Self::{variant} => {}_PAINT_PROPERTIES,",
            to_screaming_snake_case(name)
        )?;
    }
    writeln!(
        code,
        "        Self::ColorRelief => COLOR_RELIEF_PAINT_PROPERTIES,\n        Self::Custom => &[],\n    }} }}\n\
         \x20   /// Layout property metadata slice for this layer type.\n\
         \x20   pub fn layout_properties(self) -> &'static [SpecPropertyMeta] {{ match self {{"
    )?;
    for (name, variant) in &v8_layers {
        writeln!(
            code,
            "        Self::{variant} => {}_LAYOUT_PROPERTIES,",
            to_screaming_snake_case(name)
        )?;
    }
    writeln!(
        code,
        "        Self::ColorRelief => COLOR_RELIEF_LAYOUT_PROPERTIES,\n        Self::Custom => &[],\n    }} }}\n}}\n\n\
         impl std::fmt::Display for SpecLayerType {{\n\
         \x20   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ f.write_str(self.as_str()) }}\n\
         }}\n\nimpl std::str::FromStr for SpecLayerType {{\n    type Err = UnknownSpecIdentifier;\n\
         \x20   fn from_str(s: &str) -> Result<Self, Self::Err> {{ match s {{"
    )?;
    for (name, variant) in &all_layers {
        writeln!(code, "        {name:?} => Ok(Self::{variant}),")?;
    }
    writeln!(
        code,
        "        _ => Err(UnknownSpecIdentifier(s.to_owned())),\n    }} }}\n}}\n"
    )?;

    for (layer_name, layer_variant) in &v8_layers {
        for (prefix, label, variant) in
            [("paint", "paint", "Paint"), ("layout", "layout", "Layout")]
        {
            let key = format!("{prefix}_{layer_name}");
            let table = root
                .get(&key)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("missing `{key}` table in style-spec-v8.json"))?;
            emit_property_table_and_enum(
                &mut code,
                layer_name,
                layer_variant,
                label,
                variant,
                table,
            )?;
        }
    }

    let color_relief_paint: Map<String, Value> = serde_json::from_str(
        r#"{"color-relief-color":{"type":"color","transition":false,"expression":{"interpolated":true,"parameters":["elevation"]},"doc":"Elevation-driven color ramp."},"color-relief-opacity":{"type":"number","default":1,"transition":true,"expression":{"interpolated":true,"parameters":["zoom"]},"doc":"Layer opacity."}}"#,
    )?;
    emit_property_table_and_enum(
        &mut code,
        "color-relief",
        "ColorRelief",
        "paint",
        "Paint",
        &color_relief_paint,
    )?;
    let color_relief_layout: Map<String, Value> = serde_json::from_str(
        r#"{"visibility":{"type":"enum","values":{"visible":{},"none":{}},"default":"visible","doc":"Whether this layer is displayed."}}"#,
    )?;
    emit_property_table_and_enum(
        &mut code,
        "color-relief",
        "ColorRelief",
        "layout",
        "Layout",
        &color_relief_layout,
    )?;

    fs::write(
        Path::new(&env::var("OUT_DIR")?).join("style_spec_v8.rs"),
        code,
    )?;
    Ok(())
}

fn main() -> BuildResult {
    writeln!(
        io::stdout().lock(),
        "cargo:rustc-check-cfg=cfg(static_tiles_found)"
    )?;
    validate_project_wgsl_blocking()?;
    #[cfg(feature = "embed-static-tiles")]
    embed_tiles_statically_blocking()?;
    generate_protobuf_blocking()?;
    generate_style_spec_blocking()
}
