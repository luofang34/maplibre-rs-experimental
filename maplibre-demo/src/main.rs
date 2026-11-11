#![deny(unused_imports)]

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "headless")]
use maplibre::coords::LatLon;
use maplibre::{
    projection::{ProjectionSpecification, ProjectionType},
    render::settings::WgpuSettings,
    style::{source::Source, Style},
};
#[cfg(any(feature = "headless", test))]
mod coordinates;
#[cfg(feature = "headless")]
use coordinates::parse_lat_long;
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

#[cfg(feature = "headless")]
mod headless;
#[cfg(feature = "headless")]
mod snapshot;

/// World-scale style rendered on the globe, bundled so the demo works without any arguments.
const GLOBE_STYLE: &str = include_str!("../res/globe.json");
const TERRAIN_STYLE: &str = include_str!("../res/terrain.json");
const AIRSPACE_VFR_STYLE: &str = include_str!("../res/airspace_vfr.json");
/// Pitch limit of the terrain / 3D airspace demos, matching the GL JS 3d-terrain example's `maxPitch`.
const TERRAIN_MAX_PITCH_DEGREES: f64 = 85.0;

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
        /// Path to a MapLibre style JSON file. Defaults to the built-in style.
        #[clap(long, conflicts_with = "globe")]
        style: Option<PathBuf>,
        /// Use the bundled world style rendered on the globe.
        #[clap(long)]
        globe: bool,
        /// Use the bundled 3D terrain style: OpenStreetMap imagery draped over elevation tiles.
        #[clap(long, conflicts_with_all = ["globe", "style"])]
        terrain: bool,
        /// Use the bundled VFR Sectional 3D NASR airspace style over 3D terrain on the globe.
        #[clap(long, conflicts_with_all = ["globe", "terrain", "style"])]
        airspace: bool,
        /// Local `.pmtiles` file path or `https://...` / `pmtiles://...` URL overriding `sources.airspace`.
        #[clap(long)]
        airspace_pmtiles: Option<String>,
        /// Largest camera pitch in degrees. Defaults to 60, or 85 with `--terrain` / `--airspace`.
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
        /// Path to a MapLibre style JSON file.
        #[clap(long, conflicts_with_all = ["globe", "terrain", "airspace"])]
        style: Option<PathBuf>,
        /// Use the bundled world style rendered on the globe.
        #[clap(long)]
        globe: bool,
        /// Use the bundled 3D terrain style.
        #[clap(long, conflicts_with_all = ["globe", "airspace"])]
        terrain: bool,
        /// Use the bundled VFR Sectional 3D NASR airspace style over 3D terrain on the globe.
        #[clap(long, conflicts_with_all = ["globe", "terrain"])]
        airspace: bool,
        /// Local `.pmtiles` file path or `https://...` / `pmtiles://...` URL overriding `sources.airspace`.
        #[clap(long)]
        airspace_pmtiles: Option<String>,
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

/// Which bundled style a run starts from when no style path is given.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BundledStyle {
    Default,
    Globe,
    Terrain,
    AirspaceVfr,
}

fn pmtiles_source_url(spec: &str) -> Result<String, Box<dyn std::error::Error>> {
    if spec.starts_with("pmtiles://") {
        return Ok(spec.to_owned());
    }
    if spec.starts_with("https://") || spec.starts_with("http://") || spec.starts_with("file://") {
        return Ok(format!("pmtiles://{spec}"));
    }
    let abs = std::fs::canonicalize(spec)
        .map_err(|err| format!("cannot resolve airspace PMTiles path '{spec}': {err}"))?;
    Ok(format!("pmtiles://file://{}", abs.display()))
}

fn load_style(
    path: Option<&PathBuf>,
    bundled: BundledStyle,
    projection: Option<ProjectionArg>,
    airspace_pmtiles: Option<&str>,
) -> Result<Style, Box<dyn std::error::Error>> {
    let mut style = match (path, bundled) {
        (Some(path), _) => {
            let json = std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read style {}: {error}", path.display()))?;
            serde_json::from_str::<Style>(&json)
                .map_err(|error| format!("cannot parse style {}: {error}", path.display()))?
        }
        (None, BundledStyle::Globe) => serde_json::from_str::<Style>(GLOBE_STYLE)?,
        (None, BundledStyle::Terrain) => serde_json::from_str::<Style>(TERRAIN_STYLE)?,
        (None, BundledStyle::AirspaceVfr) => serde_json::from_str::<Style>(AIRSPACE_VFR_STYLE)?,
        (None, BundledStyle::Default) => Style::default(),
    };
    if let Some(pmtiles_override) = airspace_pmtiles {
        let resolved_url = pmtiles_source_url(pmtiles_override)?;
        if let Some(Source::Vector(vec_src)) = style.sources.get_mut("airspace") {
            vec_src.url = Some(resolved_url);
            vec_src.tiles = None;
        }
    }
    if path.is_some() || bundled != BundledStyle::Default {
        // Layer order becomes render order; index 0 is reserved for the depth clear.
        for (index, layer) in style.layers.iter_mut().enumerate() {
            layer.index = index as u32 + 1;
        }
    }
    if let Some(projection) = projection {
        style.projection = Some(ProjectionSpecification {
            projection_type: projection.into(),
        });
    }
    Ok(style)
}

fn select_bundled(terrain: bool, airspace: bool, globe: bool) -> BundledStyle {
    if airspace {
        BundledStyle::AirspaceVfr
    } else if terrain {
        BundledStyle::Terrain
    } else if globe {
        BundledStyle::Globe
    } else {
        BundledStyle::Default
    }
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
            airspace,
            airspace_pmtiles,
            max_pitch,
            projection,
            frames,
            debug_tiles,
            zoom_sensitivity,
        } => {
            let bundled = select_bundled(*terrain, *airspace, *globe);
            let style = load_style(
                style.as_ref(),
                bundled,
                *projection,
                airspace_pmtiles.as_deref(),
            )?;
            let default_max_pitch = if *terrain || *airspace {
                TERRAIN_MAX_PITCH_DEGREES
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
                style,
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
            airspace,
            airspace_pmtiles,
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
            let bundled = select_bundled(*terrain, *airspace, *globe);
            let loaded_style = load_style(
                style.as_ref(),
                bundled,
                *projection,
                airspace_pmtiles.as_deref(),
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
    use super::*;

    #[test]
    fn bundled_airspace_vfr_style_parses_with_zero_validation_warnings() {
        let style = load_style(None, BundledStyle::AirspaceVfr, None, None).expect("parse style");
        assert_eq!(style.validate(), Ok(()));
    }
}
