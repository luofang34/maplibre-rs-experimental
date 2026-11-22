//! Lifecycle telemetry and artifact export helpers for `render-tests`.

use std::path::Path;

use serde::Serialize;

use crate::CaseKind;

/// Structured per-fixture telemetry captured during a harness run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct FixtureTelemetry {
    pub(super) id: String,
    pub(super) kind: &'static str,
    pub(super) center: Option<[f64; 2]>,
    pub(super) zoom: Option<f64>,
    pub(super) pitch: Option<f64>,
    pub(super) bearing: Option<f64>,
    pub(super) fov_deg: Option<f64>,
    pub(super) center_elevation_m: Option<f64>,
    pub(super) required_tile_count: usize,
    pub(super) dem_tile_count: usize,
    pub(super) paused_source_count: usize,
    pub(super) frame_count: u8,
    pub(super) draws: u64,
    pub(super) state_changes: u64,
    pub(super) mean_diff: Option<f64>,
    pub(super) solid_diff: Option<u64>,
    pub(super) passed: bool,
    pub(super) ignored: bool,
}

impl FixtureTelemetry {
    /// Builds minimal telemetry when a fixture fails before rendering finishes.
    pub(super) fn fallback(id: String, kind: CaseKind, ignored: bool) -> Self {
        Self {
            id,
            kind: match kind {
                CaseKind::Render => "render",
                CaseKind::Query => "query",
            },
            center: None,
            zoom: None,
            pitch: None,
            bearing: None,
            fov_deg: None,
            center_elevation_m: None,
            required_tile_count: 0,
            dem_tile_count: 0,
            paused_source_count: 0,
            frame_count: 0,
            draws: 0,
            state_changes: 0,
            mean_diff: None,
            solid_diff: None,
            passed: false,
            ignored,
        }
    }
}

#[derive(Serialize)]
struct LifecycleReport<'a> {
    fixtures: Vec<&'a FixtureTelemetry>,
}

/// Writes `{ "fixtures": [...] }` sorted deterministically by fixture ID.
pub(super) fn write_lifecycle_json(
    path: &Path,
    fixtures: &[FixtureTelemetry],
) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Cannot create directory {}: {err}", parent.display()))?;
    }
    let mut sorted: Vec<&FixtureTelemetry> = fixtures.iter().collect();
    sorted.sort_by(|left, right| left.id.cmp(&right.id));
    let mut json = serde_json::to_string_pretty(&LifecycleReport { fixtures: sorted })
        .map_err(|err| format!("Cannot serialize lifecycle JSON: {err}"))?;
    json.push('\n');
    std::fs::write(path, json).map_err(|err| format!("Cannot write {}: {err}", path.display()))
}

/// Converts a slash-separated fixture ID into a flat filename slug.
pub(super) fn artifact_slug(id: &str) -> String {
    id.replace('/', "__")
}

/// Ensures the output artifact directory exists before any fixture runs.
pub(super) fn ensure_artifact_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("Cannot create artifact directory {}: {err}", dir.display()))
}

/// Copies `actual.png` and `diff.png` of a render fixture into `artifact_dir`.
pub(super) fn copy_fixture_artifacts(
    artifact_dir: &Path,
    test_dir: &Path,
    id: &str,
    kind: CaseKind,
) -> Result<(), String> {
    ensure_artifact_dir(artifact_dir)?;
    if kind != CaseKind::Render {
        return Ok(());
    }
    let slug = artifact_slug(id);
    for file_name in ["actual.png", "diff.png"] {
        let source = test_dir.join(file_name);
        if !source.is_file() {
            continue;
        }
        let destination = artifact_dir.join(format!("{slug}-{file_name}"));
        std::fs::copy(&source, &destination).map_err(|err| {
            format!(
                "Cannot copy artifact {} to {}: {err}",
                source.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}
