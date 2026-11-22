//! Render and query conformance harness for `maplibre-rs`.
//!
//! Runs render tests (`expected*.png`) and query tests (`expected.json`) from
//! `render-tests/src/tests/`, writes `actual.png` / `diff.png` / `actual.json`,
//! generates `render-tests/src/templates/results.html`, and gates CI via
//! `render-tests/ignores.json` when `--ci` is passed.

#![cfg_attr(
    not(test),
    forbid(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented
    )
)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use maplibre::platform::run_multithreaded;
use serde_json::Value;

mod artifacts;
mod camera_options;
mod ci;
mod comparison;
mod image_sources;
mod operations;
mod paths;
mod pattern_images;
mod queries;
mod render_case;
mod report;
mod source_loading;
mod source_tiles;
mod symbol_assets;
mod tilesets;
mod transitions;

use artifacts::{
    copy_fixture_artifacts, ensure_artifact_dir, write_lifecycle_json, FixtureTelemetry,
};
use ci::{
    build_updated_ignores, evaluate_ci_for_options, parse_cli_args, write_summaries, CliOptions,
    IgnoreFile, SuiteSummary,
};
use paths::{collect_tests, filter_tests, fixture_id, workspace_templates_dir};
#[cfg(test)]
use render_case::run_test_inner;
use render_case::run_test_with_telemetry;
use report::generate_report;

// ---------------------------------------------------------------------------
// Test metadata
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TestMeta {
    width: u32,
    height: u32,
    comparison_background: Option<[u8; 3]>,
    max_diff: f64,
    /// Largest pitch the fixture may request, as GL JS passes `maxPitch` to the map.
    max_pitch: Option<f64>,
    /// Physical pixels per logical pixel; the reference image is that many times larger.
    pixel_ratio: f64,
    /// Whether the padding edges and padded center are drawn over the frame.
    show_padding: bool,
    /// Whether each layer counts its draws instead of shading, as `showOverdrawInspector` asks.
    show_overdraw: bool,
    /// The test metadata of a query fixture, which carries `queryGeometry`.
    query: Option<serde_json::Map<String, Value>>,
    /// Whether terrain tiles hang skirts, from `terrainSkirtLength`.
    terrain_skirts: maplibre::terrain::TerrainSkirts,
    /// Camera padding left by the fixture's operations.
    padding: camera_options::Padding,
    /// Elevation the fixture pins the camera's center at.
    center_elevation: Option<f64>,
    /// Vertical field of view in degrees the fixture's operations leave.
    vertical_field_of_view: Option<f64>,
    /// Sources whose tiles stop loading, with the zoom the camera was at when they were paused.
    paused_tiles: std::collections::HashMap<String, f64>,
    /// The zoom the last `setZoom` left, and when.
    zoom_change: Option<operations::ZoomChange>,
    /// Milliseconds the fixture waits in all.
    waited: f64,
}

impl Default for TestMeta {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            comparison_background: None,
            max_diff: 0.02,
            max_pitch: None,
            pixel_ratio: 1.0,
            show_padding: false,
            show_overdraw: false,
            terrain_skirts: maplibre::terrain::TerrainSkirts::Auto,
            query: None,
            padding: camera_options::Padding::default(),
            center_elevation: None,
            vertical_field_of_view: None,
            paused_tiles: std::collections::HashMap::new(),
            zoom_change: None,
            waited: 0.0,
        }
    }
}

fn parse_test_meta(style_value: &Value) -> TestMeta {
    let test = style_value
        .pointer("/metadata/test")
        .and_then(|v| v.as_object());

    let Some(test) = test else {
        return TestMeta::default();
    };

    TestMeta {
        width: test.get("width").and_then(|v| v.as_u64()).unwrap_or(512) as u32,
        height: test.get("height").and_then(|v| v.as_u64()).unwrap_or(512) as u32,
        comparison_background: test
            .get("comparison-background")
            .and_then(Value::as_array)
            .and_then(|channels| match channels.as_slice() {
                [red, green, blue] => Some([
                    u8::try_from(red.as_u64()?).ok()?,
                    u8::try_from(green.as_u64()?).ok()?,
                    u8::try_from(blue.as_u64()?).ok()?,
                ]),
                _ => None,
            }),
        // A fixture that allows more difference than the default says so as `allowed`.
        max_diff: test
            .get("max-diff")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                test.get("allowed")
                    .and_then(Value::as_f64)
                    .map_or(0.02, |allowed| allowed.max(0.02))
            }),
        max_pitch: test.get("maxPitch").and_then(Value::as_f64),
        pixel_ratio: test
            .get("pixelRatio")
            .and_then(Value::as_f64)
            .filter(|ratio| *ratio > 0.0)
            .unwrap_or(1.0),
        show_padding: test
            .get("showPadding")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        show_overdraw: test
            .get("showOverdrawInspector")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        query: test.contains_key("queryGeometry").then(|| test.clone()),
        terrain_skirts: match test.get("terrainSkirtLength").and_then(Value::as_str) {
            Some("none") => maplibre::terrain::TerrainSkirts::None,
            _ => maplibre::terrain::TerrainSkirts::Auto,
        },
        padding: camera_options::Padding::default(),
        center_elevation: None,
        vertical_field_of_view: None,
        paused_tiles: std::collections::HashMap::new(),
        zoom_change: None,
        waited: 0.0,
    }
}

// ---------------------------------------------------------------------------
// Single test
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaseKind {
    Render,
    Query,
}

#[derive(Debug)]
struct TestOutcome {
    id: String,
    kind: CaseKind,
    result: TestResult,
    attempts: u8,
}

#[derive(Debug)]
enum TestResult {
    Pass(comparison::Difference),
    Fail(comparison::Difference),
    Error(String),
}

/// Run one test in `test_dir`. Retries unexpected failures up to 3 times to filter out
/// transient software-GPU hiccups while skipping redundant retries on known ignored cases.
async fn run_test(
    test_dir: PathBuf,
    id: String,
    retry_on_fail: bool,
    ignored: bool,
) -> (TestOutcome, FixtureTelemetry) {
    let kind = if test_dir.join("expected.json").is_file() {
        CaseKind::Query
    } else {
        CaseKind::Render
    };
    let (mut result, mut telemetry) = run_test_isolated(&test_dir);
    let mut attempts = 1_u8;
    while retry_on_fail && matches!(result, TestResult::Fail(_)) && attempts < 3 {
        attempts = attempts.wrapping_add(1);
        (result, telemetry) = run_test_isolated(&test_dir);
    }
    let mut telemetry =
        telemetry.unwrap_or_else(|| FixtureTelemetry::fallback(id.clone(), kind, ignored));
    telemetry.id = id.clone();
    telemetry.ignored = ignored;
    telemetry.passed = matches!(result, TestResult::Pass(_));
    (
        TestOutcome {
            id,
            kind,
            result,
            attempts,
        },
        telemetry,
    )
}

/// Runs one test on the current runtime and turns a panic into an error result, so one bad
/// fixture cannot abort the whole corpus.
fn run_test_isolated(test_dir: &Path) -> (TestResult, Option<FixtureTelemetry>) {
    let outcome = tokio::task::block_in_place(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tokio::runtime::Handle::current().block_on(run_test_with_telemetry(test_dir))
        }))
    });
    match outcome {
        Ok(pair) => pair,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            (TestResult::Error(format!("panicked: {message}")), None)
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn run() -> Result<bool, String> {
    let options = parse_cli_args(std::env::args().skip(1))?;

    if !options.test_root.exists() {
        return Err(format!(
            "Test directory not found: {}. Usage: cargo run -p render-tests [--ci] [test-dir]",
            options.test_root.display()
        ));
    }

    let collected = collect_tests(&options.test_root);
    if collected.is_empty() {
        return Err(format!("No tests found in {}", options.test_root.display()));
    }
    let tests = filter_tests(collected, &options.test_root, &options.filters)?;

    let existing_ignores =
        if options.ci || (options.write_ignores && options.ignores_path.is_file()) {
            Some(IgnoreFile::load(&options.ignores_path)?)
        } else {
            None
        };

    run_multithreaded(run_tests(options, tests, existing_ignores))
}

fn log_case_outcome(
    outcome: &TestOutcome,
    id: &str,
    ci_ignored: bool,
    elapsed: std::time::Duration,
) {
    let tag = match (&outcome.result, ci_ignored) {
        (TestResult::Pass(difference), false) => format!("PASS  ({difference})"),
        (TestResult::Pass(difference), true) => format!("UPASS ({difference})"),
        (TestResult::Fail(difference), true) => format!("IGN   ({difference})"),
        (TestResult::Error(msg), true) => format!("IGN   {msg}"),
        (TestResult::Fail(difference), false) => format!("FAIL  ({difference})"),
        (TestResult::Error(msg), false) => format!("ERR   {msg}"),
    };
    let retry_label = if outcome.attempts > 1 {
        format!(" [attempt {}]", outcome.attempts)
    } else {
        String::new()
    };
    tracing::info!("  {tag}  {id}{retry_label}  ({elapsed:.1?})");
}

async fn execute_tests(
    options: &CliOptions,
    tests: &[PathBuf],
    active_ignores: &BTreeMap<String, String>,
) -> Result<(Vec<TestOutcome>, Vec<FixtureTelemetry>), String> {
    if let Some(artifact_dir) = options.artifact_dir.as_deref() {
        ensure_artifact_dir(artifact_dir)?;
    }
    let mut outcomes = Vec::with_capacity(tests.len());
    let mut telemetries = Vec::with_capacity(tests.len());
    for test_dir in tests {
        let id = fixture_id(test_dir, &options.test_root);
        let is_ignored = active_ignores.contains_key(&id);
        let retry_on_fail = !(options.ci && is_ignored);
        let start = Instant::now();
        let (outcome, telemetry) =
            run_test(test_dir.clone(), id.clone(), retry_on_fail, is_ignored).await;
        log_case_outcome(&outcome, &id, options.ci && is_ignored, start.elapsed());
        if let Some(artifact_dir) = options.artifact_dir.as_deref() {
            copy_fixture_artifacts(artifact_dir, test_dir, &id, outcome.kind)?;
        }
        outcomes.push(outcome);
        telemetries.push(telemetry);
    }
    Ok((outcomes, telemetries))
}

fn log_ci_diagnostics(summary: &SuiteSummary, options: &CliOptions) {
    for failure in &summary.unexpected_failures {
        tracing::error!("Unexpected failure: {failure}");
    }
    for id in &summary.unexpected_passes {
        tracing::error!(
            "Unexpected pass on ignored fixture `{id}` — remove it from {}",
            options.ignores_path.display()
        );
    }
    for id in &summary.stale_ignores {
        tracing::error!(
            "Stale ignore entry `{id}` in {} does not match any fixture",
            options.ignores_path.display()
        );
    }
}

async fn run_tests(
    options: CliOptions,
    tests: Vec<PathBuf>,
    existing_ignores: Option<IgnoreFile>,
) -> Result<bool, String> {
    tracing::info!(
        "Running {} conformance tests from {}",
        tests.len(),
        options.test_root.display()
    );
    tracing::info!("{}", "-".repeat(70));
    let active_ignores = existing_ignores
        .as_ref()
        .map(|file| file.ignores.clone())
        .unwrap_or_default();
    let (outcomes, telemetries) = execute_tests(&options, &tests, &active_ignores).await?;

    let report_path = generate_report(&outcomes, &workspace_templates_dir())?;
    tracing::info!("Report written to: {}", report_path.display());

    if options.write_ignores {
        let updated = build_updated_ignores(&outcomes, existing_ignores.as_ref());
        updated.save(&options.ignores_path)?;
        tracing::info!(
            "Wrote {} ignore entries to {}",
            updated.ignores.len(),
            options.ignores_path.display()
        );
    }

    let summary = evaluate_ci_for_options(&outcomes, &active_ignores, &options);
    write_summaries(
        &summary,
        options.summary_json.as_deref(),
        options.summary_md.as_deref(),
    )?;
    if let Some(lifecycle_path) = options.lifecycle_json.as_deref() {
        write_lifecycle_json(lifecycle_path, &telemetries)?;
    }

    tracing::info!("{}", "-".repeat(70));
    tracing::info!(
        "Summary: {}/{} passed ({:.2}%), {} ignored, {} unexpected failures, {} unexpected passes",
        summary.total.passed,
        summary.total.total,
        summary.total.pass_rate_percent,
        summary.total.ignored,
        summary.unexpected_failures.len(),
        summary.unexpected_passes.len(),
    );

    if options.ci {
        log_ci_diagnostics(&summary, &options);
        return Ok(summary.ci_passed());
    }
    if options.write_ignores {
        return Ok(true);
    }
    Ok(summary.total.ignored == 0 && summary.unexpected_failures.is_empty())
}

fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
    }
}
