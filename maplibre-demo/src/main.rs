#![deny(unused_imports)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "headless")]
use maplibre::coords::LatLon;
use maplibre::{projection::ProjectionType, render::settings::WgpuSettings};
#[cfg(any(feature = "headless", test))]
mod coordinates;
#[cfg(feature = "headless")]
use coordinates::parse_lat_long;
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

#[cfg(feature = "headless")]
mod headless;
#[cfg(feature = "headless")]
mod snapshot;
mod style_loader;

use style_loader::{load_style, select_bundled, style_uses_3d_elevation};

/// Pitch limit for styles with 3D terrain or extruded volumes, matching GL JS 3d-terrain `maxPitch`.
const ELEVATED_MAX_PITCH_DEGREES: f64 = 85.0;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProjectionArg {
    Globe,
    Mercator,
    VerticalPerspective,
}

impl From<ProjectionArg> for ProjectionType {
    fn from(projection: ProjectionArg) -> Self {
        match projection {
            ProjectionArg::Globe => ProjectionType::Globe,
            ProjectionArg::Mercator => ProjectionType::Mercator,
            ProjectionArg::VerticalPerspective => ProjectionType::VerticalPerspective,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Opens an interactive map window.
    ///
    /// Controls: left-drag pans, wheel or +/- zooms around the pointer, right-drag rotates and
    /// tilts, W/A/S/D or the arrow keys pan, Escape quits.
    Headed {
        /// Local file path or `http(s)://` / `file://` URL to a MapLibre style JSON document.
        #[clap(long, conflicts_with_all = ["globe", "terrain"])]
        style: Option<String>,
        /// Use the bundled world style rendered on the globe.
        #[clap(long)]
        globe: bool,
        /// Use the bundled 3D terrain style: OpenStreetMap imagery draped over elevation tiles.
        #[clap(long, conflicts_with = "globe")]
        terrain: bool,
        /// Override a style source's URL (`NAME=URL_OR_PATH`, repeatable).
        #[clap(long = "source-url", value_name = "NAME=URL_OR_PATH")]
        source_urls: Vec<String>,
        /// Largest camera pitch in degrees. Defaults to 85 for 3D terrain/extrusion styles, else 60.
        #[clap(long)]
        max_pitch: Option<f64>,
        /// Override the projection declared by the style.
        #[clap(long, value_enum)]
        projection: Option<ProjectionArg>,
        /// Exit after rendering this many frames.
        #[clap(long)]
        frames: Option<u64>,
        /// Outline every tile of the view pattern in red, to inspect tile selection.
        #[clap(long)]
        debug_tiles: bool,
        /// Multiplier on scroll-wheel / trackpad zoom rate (`1.0` = default).
        #[clap(long, default_value_t = 1.0)]
        zoom_sensitivity: f64,
    },
    #[cfg(feature = "headless")]
    Headless {
        #[clap(default_value_t = 400)]
        tile_size: u32,
        #[clap(
            value_parser = clap::builder::ValueParser::new(parse_lat_long),
            default_value_t = LatLon::new(48.0345697188, 11.3475219363)
        )]
        min: LatLon,
        #[clap(
            value_parser = clap::builder::ValueParser::new(parse_lat_long),
            default_value_t = LatLon::new(48.255861, 11.7917815798)
        )]
        max: LatLon,
    },
    /// Renders a single high-resolution offscreen PNG frame once all view tiles settle.
    #[cfg(feature = "headless")]
    Snapshot {
        /// Local file path or `http(s)://` / `file://` URL to a MapLibre style JSON document.
        #[clap(long, conflicts_with_all = ["globe", "terrain"])]
        style: Option<String>,
        /// Use the bundled world style rendered on the globe.
        #[clap(long)]
        globe: bool,
        /// Use the bundled 3D terrain style.
        #[clap(long, conflicts_with = "globe")]
        terrain: bool,
        /// Override a style source's URL (`NAME=URL_OR_PATH`, repeatable).
        #[clap(long = "source-url", value_name = "NAME=URL_OR_PATH")]
        source_urls: Vec<String>,
        /// Override the projection declared by the style.
        #[clap(long, value_enum)]
        projection: Option<ProjectionArg>,
        #[clap(long, default_value_t = 1280)]
        width: u32,
        #[clap(long, default_value_t = 800)]
        height: u32,
        #[clap(long, allow_hyphen_values = true)]
        lat: Option<f64>,
        #[clap(long, allow_hyphen_values = true)]
        lon: Option<f64>,
        #[clap(long)]
        zoom: Option<f64>,
        #[clap(long, allow_hyphen_values = true)]
        pitch: Option<f64>,
        #[clap(long, allow_hyphen_values = true)]
        bearing: Option<f64>,
        #[clap(long, default_value_t = 360)]
        max_frames: usize,
        #[clap(long, default_value_t = 30)]
        timeout_secs: u64,
        /// Output PNG path.
        #[clap(long)]
        out: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Captures both tracing events and `log` records; RUST_LOG selects the level.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    #[cfg(feature = "trace")]
    maplibre::platform::trace::enable_tracing();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Headed {
            style,
            globe,
            terrain,
            source_urls,
            max_pitch,
            projection,
            frames,
            debug_tiles,
            zoom_sensitivity,
        } => {
            let bundled = select_bundled(*terrain, *globe);
            let loaded_style = load_style(
                style.as_deref(),
                bundled,
                projection.map(Into::into),
                source_urls,
            )?;
            let default_max_pitch = if style_uses_3d_elevation(&loaded_style) {
                ELEVATED_MAX_PITCH_DEGREES
            } else {
                HeadedMapOptions::default().max_pitch_degrees
            };
            run_headed_map(
                Some(PathBuf::from("./maplibre-cache".to_string())),
                WinitMapWindowConfig::new("maplibre".to_string()),
                WgpuSettings {
                    backends: Some(maplibre::render::settings::Backends::all()),
                    ..WgpuSettings::default()
                },
                loaded_style,
                HeadedMapOptions {
                    max_frames: *frames,
                    max_pitch_degrees: max_pitch.unwrap_or(default_max_pitch),
                    debug_tiles: *debug_tiles,
                    zoom_sensitivity: *zoom_sensitivity,
                    ..HeadedMapOptions::default()
                },
            )?;
        }
        #[cfg(feature = "headless")]
        Commands::Headless {
            tile_size,
            min,
            max,
        } => {
            maplibre::platform::run_multithreaded(async {
                headless::run_headless(*tile_size, *min, *max).await
            })?;
        }
        #[cfg(feature = "headless")]
        Commands::Snapshot {
            style,
            globe,
            terrain,
            source_urls,
            projection,
            width,
            height,
            lat,
            lon,
            zoom,
            pitch,
            bearing,
            max_frames,
            timeout_secs,
            out,
        } => {
            let bundled = select_bundled(*terrain, *globe);
            let loaded_style = load_style(
                style.as_deref(),
                bundled,
                projection.map(Into::into),
                source_urls,
            )?;
            let center = match (*lat, *lon) {
                (Some(lat), Some(lon)) => Some(LatLon::new(lat, lon)),
                _ => None,
            };
            maplibre::platform::run_multithreaded(async {
                snapshot::render_snapshot(snapshot::SnapshotOptions {
                    style: loaded_style,
                    width: *width,
                    height: *height,
                    center,
                    zoom: *zoom,
                    pitch: *pitch,
                    bearing: *bearing,
                    max_frames: *max_frames,
                    timeout_secs: *timeout_secs,
                    out_path: out,
                })
                .await
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use maplibre::style::{source::Source, Style};

    use crate::style_loader::{
        apply_source_url_overrides, parent_uri_base, resolve_relative_source_urls,
        style_uses_3d_elevation, BundledStyle,
    };

    #[test]
    fn resolves_relative_pmtiles_and_http_urls_against_style_base() {
        let mut style: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "sources": {
                "volumes": {"type": "vector", "url": "pmtiles://../cycles/current/volumes.pmtiles"},
                "local_adj": {"type": "vector", "url": "pmtiles://./volumes.pmtiles"}
            },
            "layers": []
        }))
        .expect("parse style");

        let base = parent_uri_base("https://example.org/repo/styles/default.json")
            .expect("parent URI base");
        assert_eq!(base, "https://example.org/repo/styles/");
        resolve_relative_source_urls(&mut style, &base);

        let Some(Source::Vector(v1)) = style.sources.get("volumes") else {
            panic!("missing volumes source");
        };
        assert_eq!(
            v1.url.as_deref(),
            Some("pmtiles://https://example.org/repo/cycles/current/volumes.pmtiles")
        );

        let Some(Source::Vector(v2)) = style.sources.get("local_adj") else {
            panic!("missing local_adj source");
        };
        assert_eq!(
            v2.url.as_deref(),
            Some("pmtiles://https://example.org/repo/styles/volumes.pmtiles")
        );
    }

    #[test]
    fn source_url_override_rewrites_named_source_and_rejects_unknown() {
        let mut style: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "sources": {
                "volumes": {"type": "vector", "tiles": ["https://a.invalid/{z}/{x}/{y}.pbf"]}
            },
            "layers": []
        }))
        .expect("parse style");

        apply_source_url_overrides(
            &mut style,
            &["volumes=https://cdn.example.org/data.pmtiles".to_owned()],
        )
        .expect("override valid source");

        let Some(Source::Vector(vec_src)) = style.sources.get("volumes") else {
            panic!("missing volumes source");
        };
        assert_eq!(
            vec_src.url.as_deref(),
            Some("pmtiles://https://cdn.example.org/data.pmtiles")
        );
        assert!(vec_src.tiles.is_none());

        assert!(apply_source_url_overrides(
            &mut style,
            &["missing=https://cdn.example.org/x".into()]
        )
        .is_err());
    }

    #[test]
    fn bundled_terrain_and_extrusion_styles_use_elevated_pitch_limit() {
        let terrain =
            crate::style_loader::load_style(None, BundledStyle::Terrain, None, &[]).expect("load");
        assert!(style_uses_3d_elevation(&terrain));

        let globe =
            crate::style_loader::load_style(None, BundledStyle::Globe, None, &[]).expect("load");
        assert!(!style_uses_3d_elevation(&globe));

        let extruded: Style = serde_json::from_value(serde_json::json!({
            "version": 8,
            "sources": {
                "v": {"type": "vector", "tiles": ["https://a.invalid/{z}/{x}/{y}.pbf"]}
            },
            "layers": [{
                "id": "prism",
                "type": "fill-extrusion",
                "source": "v",
                "source-layer": "prism",
                "paint": {
                    "fill-extrusion-part": "walls",
                    "fill-extrusion-height": 500.0,
                    "fill-extrusion-height-datum": "msl"
                }
            }]
        }))
        .expect("parse extrusion style");
        assert_eq!(extruded.validate(), Ok(()));
        assert!(style_uses_3d_elevation(&extruded));
    }
}
