//! HTML and case-list report output.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{CaseKind, TestOutcome, TestResult};

pub(super) fn generate_report(
    outcomes: &[TestOutcome],
    templates_dir: &Path,
) -> Result<PathBuf, String> {
    let report_template_path = templates_dir.join("report_template.html");
    let item_template_path = templates_dir.join("result_item_template.html");
    let results_path = templates_dir.join("results.html");
    let failed_ids_path = templates_dir.join("results-failed-caseIds.txt");
    let errored_ids_path = templates_dir.join("results-errored-caseIds.txt");
    let report_template = std::fs::read_to_string(report_template_path)
        .unwrap_or_else(|_| "<html><body>${resultData}</body></html>".to_string());
    let item_template = std::fs::read_to_string(item_template_path).unwrap_or_default();
    let (failed_items, errored_items, failed_ids, errored_ids) = report_items(outcomes);
    let result_data = item_template
        .replace("${failedItemsLength}", &failed_ids.len().to_string())
        .replace("${failedItems}", &failed_items)
        .replace("${erroredItemsLength}", &errored_ids.len().to_string())
        .replace("${erroredItems}", &errored_items);
    let banner = report_banner(outcomes, failed_ids.len(), errored_ids.len());
    let full_html = if result_data.is_empty() {
        report_template.replace("${resultData}", &banner)
    } else {
        report_template.replace("${resultData}", &format!("{banner}\n{result_data}"))
    };
    std::fs::write(&results_path, full_html)
        .map_err(|error| format!("Cannot write {}: {error}", results_path.display()))?;
    std::fs::write(&failed_ids_path, failed_ids.join("\n"))
        .map_err(|error| format!("Cannot write {}: {error}", failed_ids_path.display()))?;
    std::fs::write(&errored_ids_path, errored_ids.join("\n"))
        .map_err(|error| format!("Cannot write {}: {error}", errored_ids_path.display()))?;
    Ok(results_path)
}

fn report_items(outcomes: &[TestOutcome]) -> (String, String, Vec<String>, Vec<String>) {
    let mut failed_items = String::new();
    let mut errored_items = String::new();
    let mut failed_ids = Vec::new();
    let mut errored_ids = Vec::new();
    for outcome in outcomes {
        let item = report_item(outcome);
        let test_id = format!("tests/{}", outcome.id);
        match outcome.result {
            TestResult::Fail(_) => {
                failed_items.push_str(&item);
                failed_ids.push(test_id);
            }
            TestResult::Error(_) => {
                errored_items.push_str(&item);
                errored_ids.push(test_id);
            }
            TestResult::Pass(_) => {}
        }
    }
    (failed_items, errored_items, failed_ids, errored_ids)
}

fn report_item(outcome: &TestOutcome) -> String {
    let expected = format!("../tests/{}/expected.png", outcome.id);
    let status = match &outcome.result {
        TestResult::Pass(difference) => format!("PASS ({difference})"),
        TestResult::Fail(difference) => format!("FAIL ({difference})"),
        TestResult::Error(message) => format!("ERROR: {message}"),
    };
    format!(
        r#"<div class="test">
  <h2>{id}</h2>
  <div class="imagewrap">
    <div><p>Actual</p><img src="../tests/{id}/actual.png" data-alt-src="{expected}"></div>
    <div><p>Expected</p><img src="{expected}"></div>
    <div class="diff"><p>Diff</p><img src="../tests/{id}/diff.png"></div>
  </div>
  <p>{status}</p>
</div>"#,
        id = outcome.id,
    )
}

fn report_banner(outcomes: &[TestOutcome], failed: usize, errored: usize) -> String {
    let passed = outcomes
        .iter()
        .filter(|outcome| matches!(outcome.result, TestResult::Pass(_)))
        .count();
    if failed == 0 && errored == 0 {
        format!(r#"<h1 style="color: green">All {passed} tests passed!</h1>"#)
    } else {
        format!(
            r#"<p class="stats">{passed} passed / {failed} failed / {errored} errored out of {} total</p>"#,
            outcomes.len()
        )
    }
}

/// Structured per-fixture telemetry captured during a harness run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct FixtureTelemetry {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) center: Option<[f64; 2]>,
    pub(crate) zoom: Option<f64>,
    pub(crate) pitch: Option<f64>,
    pub(crate) bearing: Option<f64>,
    pub(crate) fov_deg: Option<f64>,
    pub(crate) center_elevation_m: Option<f64>,
    pub(crate) required_tile_count: usize,
    pub(crate) dem_tile_count: usize,
    pub(crate) paused_source_count: usize,
    pub(crate) frame_count: u8,
    pub(crate) draws: u64,
    pub(crate) state_changes: u64,
    pub(crate) mean_diff: Option<f64>,
    pub(crate) solid_diff: Option<u64>,
    pub(crate) passed: bool,
    pub(crate) ignored: bool,
}

impl FixtureTelemetry {
    /// Builds minimal telemetry when a fixture fails before rendering finishes.
    pub(crate) fn fallback(id: String, kind: CaseKind, ignored: bool) -> Self {
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
pub(crate) fn write_lifecycle_json(
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
pub(crate) fn artifact_slug(id: &str) -> String {
    id.replace('/', "__")
}

/// Ensures the output artifact directory exists before any fixture runs.
pub(crate) fn ensure_artifact_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("Cannot create artifact directory {}: {err}", dir.display()))
}

/// Copies `actual.png` and `diff.png` of a render fixture into `artifact_dir`.
pub(crate) fn copy_fixture_artifacts(
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
