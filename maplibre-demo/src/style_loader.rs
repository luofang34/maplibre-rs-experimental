use std::path::PathBuf;

use maplibre::{
    io::{resource_loader::SharedLoader, source_client::HttpClient},
    platform::http_client::ReqwestHttpClient,
    projection::{ProjectionSpecification, ProjectionType},
    style::{
        layer::LayerPaint,
        source::{GeoJsonData, Source},
        Style,
    },
};

/// World-scale style rendered on the globe, bundled so the demo works without any arguments.
const GLOBE_STYLE: &str = include_str!("../res/globe.json");
const TERRAIN_STYLE: &str = include_str!("../res/terrain.json");

/// Which bundled style a run starts from when no `--style` path or URL is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundledStyle {
    Default,
    Globe,
    Terrain,
}

pub fn select_bundled(terrain: bool, globe: bool) -> BundledStyle {
    if terrain {
        BundledStyle::Terrain
    } else if globe {
        BundledStyle::Globe
    } else {
        BundledStyle::Default
    }
}

/// Returns `true` when `style` enables 3D terrain or extrudes 3D volumes, warranting an 85°
/// default pitch ceiling in interactive navigation.
pub fn style_uses_3d_elevation(style: &Style) -> bool {
    style.terrain.is_some()
        || style
            .layers
            .iter()
            .any(|layer| matches!(layer.paint, Some(LayerPaint::FillExtrusion(_))))
}

pub fn load_style(
    spec: Option<&str>,
    bundled: BundledStyle,
    projection: Option<ProjectionType>,
    source_overrides: &[String],
) -> Result<Style, Box<dyn std::error::Error>> {
    let (mut style, base_uri) = match (spec, bundled) {
        (Some(spec), _) => {
            let (json, base_uri) = read_style_document(spec)?;
            let parsed = serde_json::from_str::<Style>(&json)
                .map_err(|error| format!("cannot parse style '{spec}': {error}"))?;
            (parsed, base_uri)
        }
        (None, BundledStyle::Globe) => (serde_json::from_str::<Style>(GLOBE_STYLE)?, None),
        (None, BundledStyle::Terrain) => (serde_json::from_str::<Style>(TERRAIN_STYLE)?, None),
        (None, BundledStyle::Default) => (Style::default(), None),
    };

    if let Some(base_uri) = base_uri.as_deref() {
        resolve_relative_source_urls(&mut style, base_uri);
    }
    apply_source_url_overrides(&mut style, source_overrides)?;

    if spec.is_some() || bundled != BundledStyle::Default {
        // Layer order becomes render order; index 0 is reserved for the depth clear.
        for (index, layer) in style.layers.iter_mut().enumerate() {
            layer.index = index as u32 + 1;
        }
    }
    if let Some(projection_type) = projection {
        style.projection = Some(ProjectionSpecification { projection_type });
    }
    Ok(style)
}

fn is_absolute_uri(uri: &str) -> bool {
    uri.starts_with("https://") || uri.starts_with("http://") || uri.starts_with("file://")
}

fn read_style_document(spec: &str) -> Result<(String, Option<String>), Box<dyn std::error::Error>> {
    if is_absolute_uri(spec) {
        let owned = spec.to_owned();
        let bytes = maplibre::platform::run_multithreaded(async move {
            let loader = SharedLoader::new(ReqwestHttpClient::new::<PathBuf>(None));
            loader.fetch(&owned).await.map_err(|err| err.describe())
        })
        .map_err(|msg| format!("cannot fetch style '{spec}': {msg}"))?;
        let text = String::from_utf8(bytes)
            .map_err(|err| format!("style '{spec}' is not valid UTF-8: {err}"))?;
        return Ok((text, parent_uri_base(spec)));
    }

    let abs = std::fs::canonicalize(spec)
        .map_err(|error| format!("cannot read style '{spec}': {error}"))?;
    let text = std::fs::read_to_string(&abs)
        .map_err(|error| format!("cannot read style {}: {error}", abs.display()))?;
    let base_uri = abs.parent().map(|dir| {
        #[cfg(windows)]
        let slash_dir = dir.to_string_lossy().replace('\\', "/");
        #[cfg(not(windows))]
        let slash_dir = dir.to_string_lossy();
        if slash_dir.starts_with('/') {
            format!("file://{slash_dir}/")
        } else {
            format!("file:///{slash_dir}/")
        }
    });
    Ok((text, base_uri))
}

pub(crate) fn parent_uri_base(uri: &str) -> Option<String> {
    let clean = uri.split(['?', '#']).next().unwrap_or(uri);
    let idx = clean.rfind('/')?;
    let scheme_end = clean.find("://").map(|i| i + 3)?;
    (idx >= scheme_end).then(|| clean[..=idx].to_owned())
}

pub(crate) fn resolve_uri_against_base(base_uri: &str, raw: &str) -> String {
    if let Some(inner) = raw.strip_prefix("pmtiles://") {
        if is_absolute_uri(inner) {
            return raw.to_owned();
        }
        return format!("pmtiles://{}", join_uri(base_uri, inner));
    }
    if is_absolute_uri(raw) || raw.starts_with("data:") {
        return raw.to_owned();
    }
    join_uri(base_uri, raw)
}

fn join_uri(base_uri: &str, rel: &str) -> String {
    let (origin, base_path) = if let Some(rest) = base_uri.strip_prefix("file://") {
        ("file://", rest)
    } else if let Some(scheme_pos) = base_uri.find("://") {
        let after_scheme = &base_uri[scheme_pos + 3..];
        match after_scheme.find('/') {
            Some(slash) => {
                let split_at = scheme_pos + 3 + slash;
                (&base_uri[..split_at], &base_uri[split_at..])
            }
            None => (base_uri, "/"),
        }
    } else {
        return rel.to_owned();
    };

    let (rel_path, suffix) = match rel.find(['?', '#']) {
        Some(pos) => (&rel[..pos], &rel[pos..]),
        None => (rel, ""),
    };

    let mut segments: Vec<&str> = if rel_path.starts_with('/') {
        Vec::new()
    } else {
        base_path.split('/').filter(|seg| !seg.is_empty()).collect()
    };

    for seg in rel_path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                let _ = segments.pop();
            }
            part => segments.push(part),
        }
    }

    format!("{origin}/{}{suffix}", segments.join("/"))
}

pub(crate) fn resolve_relative_source_urls(style: &mut Style, base_uri: &str) {
    for source in style.sources.values_mut() {
        match source {
            Source::Vector(s) | Source::Raster(s) => {
                if let Some(url) = s.url.as_mut() {
                    *url = resolve_uri_against_base(base_uri, url);
                }
                if let Some(tiles) = s.tiles.as_mut() {
                    for tile in tiles {
                        *tile = resolve_uri_against_base(base_uri, tile);
                    }
                }
            }
            Source::RasterDem(s) => {
                if let Some(url) = s.url.as_mut() {
                    *url = resolve_uri_against_base(base_uri, url);
                }
                if let Some(tiles) = s.tiles.as_mut() {
                    for tile in tiles {
                        *tile = resolve_uri_against_base(base_uri, tile);
                    }
                }
            }
            Source::GeoJson(s) => {
                if let GeoJsonData::Url(url) = &mut s.data {
                    *url = resolve_uri_against_base(base_uri, url);
                }
            }
            Source::Image(s) => {
                s.url = resolve_uri_against_base(base_uri, &s.url);
            }
        }
    }
}

pub(crate) fn normalize_source_override_spec(
    spec: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    if spec.starts_with("pmtiles://") {
        return Ok(spec.to_owned());
    }
    let path_without_query = spec.split(['?', '#']).next().unwrap_or(spec);
    let is_pmtiles = path_without_query.ends_with(".pmtiles");
    if is_absolute_uri(spec) {
        return Ok(if is_pmtiles {
            format!("pmtiles://{spec}")
        } else {
            spec.to_owned()
        });
    }
    let abs = std::fs::canonicalize(spec)
        .map_err(|err| format!("cannot resolve source path '{spec}': {err}"))?;
    #[cfg(windows)]
    let file_uri = format!("file:///{}", abs.to_string_lossy().replace('\\', "/"));
    #[cfg(not(windows))]
    let file_uri = format!("file://{}", abs.display());
    Ok(if is_pmtiles {
        format!("pmtiles://{file_uri}")
    } else {
        file_uri
    })
}

pub(crate) fn apply_source_url_overrides(
    style: &mut Style,
    overrides: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in overrides {
        let (name, spec) = entry.split_once('=').ok_or_else(|| {
            format!("invalid --source-url '{entry}'; expected <source_name>=<url_or_path>")
        })?;
        let name = name.trim();
        let spec = spec.trim();
        if name.is_empty() || spec.is_empty() {
            return Err(format!(
                "invalid --source-url '{entry}'; source name and URL must be non-empty"
            )
            .into());
        }
        let resolved = normalize_source_override_spec(spec)?;
        let source = style
            .sources
            .get_mut(name)
            .ok_or_else(|| format!("style has no source named '{name}'"))?;
        match source {
            Source::Vector(s) | Source::Raster(s) => {
                s.url = Some(resolved);
                s.tiles = None;
            }
            Source::RasterDem(s) => {
                s.url = Some(resolved);
                s.tiles = None;
            }
            Source::GeoJson(s) => {
                s.data = GeoJsonData::Url(resolved);
            }
            Source::Image(s) => {
                s.url = resolved;
            }
        }
    }
    Ok(())
}
