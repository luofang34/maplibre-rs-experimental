//! Workspace-relative paths used by the render harness.

use std::path::PathBuf;

use maplibre::{coords::WorldTileCoords, style::source::TileAddressingScheme};

pub(super) fn workspace_tests_dir() -> PathBuf {
    PathBuf::from("render-tests/src/tests")
}

pub(super) fn workspace_templates_dir() -> PathBuf {
    PathBuf::from("render-tests/src/templates")
}

pub(super) fn local_tile_path(
    template: &str,
    coords: WorldTileCoords,
    scheme: Option<TileAddressingScheme>,
) -> Result<PathBuf, String> {
    let relative = template
        .strip_prefix("local://tiles/")
        .ok_or_else(|| format!("Unsupported tile URL in render harness: {template}"))?;
    let relative = relative
        .replace("{z}", &u8::from(coords.z).to_string())
        .replace("{x}", &coords.x.to_string())
        .replace("{y}", &row_in_scheme(coords, scheme).to_string());
    Ok(PathBuf::from("render-tests/src/assets/tiles").join(relative))
}

/// The row a tile has in the addressing scheme of its source: TMS rows count from the south.
fn row_in_scheme(coords: WorldTileCoords, scheme: Option<TileAddressingScheme>) -> i64 {
    match scheme {
        Some(TileAddressingScheme::TMS) => {
            (1_i64 << u32::from(u8::from(coords.z))) - 1 - i64::from(coords.y)
        }
        _ => i64::from(coords.y),
    }
}

pub(super) fn local_data_path(url: &str) -> Result<PathBuf, String> {
    for directory in ["data", "geojson"] {
        if let Some(relative) = url.strip_prefix(&format!("local://{directory}/")) {
            return Ok(PathBuf::from("render-tests/src/assets")
                .join(directory)
                .join(relative));
        }
    }
    Err(format!("Unsupported GeoJSON URL in render harness: {url}"))
}

/// The file behind a `local://styles/...` style URL.
pub(super) fn local_style_path(url: &str) -> Result<PathBuf, String> {
    let relative = url
        .strip_prefix("local://styles/")
        .ok_or_else(|| format!("Unsupported style URL in render harness: {url}"))?;
    Ok(PathBuf::from("render-tests/src/assets/styles").join(relative))
}

pub(super) fn collect_tests(test_root: &std::path::Path) -> Vec<PathBuf> {
    let mut tests = walkdir::WalkDir::new(test_root)
        .min_depth(1)
        .max_depth(5)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() == "style.json")
        .filter_map(|entry| entry.path().parent().map(std::path::Path::to_path_buf))
        .filter(|parent| !parent.ends_with("projection/perspective"))
        .collect::<Vec<_>>();
    tests.sort();
    tests
}
