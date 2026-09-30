//! Render test harness for maplibre-rs.
//!
//! Runs render tests from `render-tests/src/tests/`, compares against
//! `expected.png`, writes `actual.png` and `diff.png`, and generates
//! `render-tests/src/templates/results.html`.
//!
//! # Usage
//!
//! ```
//! # Run all tests (from workspace root)
//! cargo run -p render-tests
//!
//! # Run a single test or category
//! cargo run -p render-tests -- render-tests/src/tests/fill-color
//! ```

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use maplibre::platform::run_multithreaded;
use serde_json::Value;

mod comparison;
mod image_sources;
mod operations;
mod paths;
mod pattern_images;
mod render_case;
mod report;
mod source_loading;
mod source_tiles;
mod symbol_assets;
mod tilesets;

use paths::{collect_tests, workspace_templates_dir, workspace_tests_dir};
use render_case::run_test_inner;
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
        max_diff: test.get("max-diff").and_then(Value::as_f64).unwrap_or(0.02),
        max_pitch: test.get("maxPitch").and_then(Value::as_f64),
        pixel_ratio: test
            .get("pixelRatio")
            .and_then(Value::as_f64)
            .filter(|ratio| *ratio > 0.0)
            .unwrap_or(1.0),
    }
}

// ---------------------------------------------------------------------------
// Single test
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TestOutcome {
    id: String,
    result: TestResult,
    attempts: u8,
}

#[derive(Debug)]
enum TestResult {
    Pass { diff: f64 },
    Fail { diff: f64 },
    Error(String),
}

/// Run one test in `test_dir`. Writes `actual.png` and `diff.png` into `test_dir`.
async fn run_test(test_dir: PathBuf) -> TestOutcome {
    let id = test_dir
        .iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned();

    let mut result = run_test_isolated(&test_dir);
    let mut attempts = 1_u8;
    while matches!(result, TestResult::Fail { .. }) && attempts < 3 {
        attempts = attempts.wrapping_add(1);
        result = run_test_isolated(&test_dir);
    }
    TestOutcome {
        id,
        result,
        attempts,
    }
}

/// Runs one test on the current runtime and turns a panic into an error result, so one bad
/// fixture cannot abort the whole corpus.
fn run_test_isolated(test_dir: &Path) -> TestResult {
    let outcome = tokio::task::block_in_place(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tokio::runtime::Handle::current().block_on(run_test_inner(test_dir))
        }))
    });
    match outcome {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            TestResult::Error(format!("panicked: {message}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn run() -> Result<bool, String> {
    let args: Vec<String> = std::env::args().collect();

    let test_root = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        workspace_tests_dir()
    };

    if !test_root.exists() {
        return Err(format!(
            "Test directory not found: {}. Usage: cargo run -p render-tests [test-dir]",
            test_root.display()
        ));
    }

    let tests = collect_tests(&test_root);
    if tests.is_empty() {
        return Err(format!("No tests found in {}", test_root.display()));
    }

    run_multithreaded(run_tests(test_root, tests))
}

async fn run_tests(test_root: PathBuf, tests: Vec<PathBuf>) -> Result<bool, String> {
    tracing::info!(
        "Running {} render tests from {}",
        tests.len(),
        test_root.display()
    );
    tracing::info!("{}", "-".repeat(70));

    let mut outcomes: Vec<TestOutcome> = Vec::new();

    for test_dir in &tests {
        let name = test_dir
            .strip_prefix(&test_root)
            .unwrap_or(test_dir)
            .display()
            .to_string();

        let start = Instant::now();
        let outcome = run_test(test_dir.clone()).await;
        let elapsed = start.elapsed();

        let tag = match &outcome.result {
            TestResult::Pass { diff } => format!("PASS  (diff={diff:.4})"),
            TestResult::Fail { diff } => format!("FAIL  (diff={diff:.4})"),
            TestResult::Error(msg) => format!("ERR   {msg}"),
        };
        let retry_label = if outcome.attempts > 1 {
            format!(" [attempt {}]", outcome.attempts)
        } else {
            String::new()
        };

        tracing::info!("  {tag}  {name}{retry_label}  ({elapsed:.1?})");

        outcomes.push(outcome);
    }

    let passed = outcomes
        .iter()
        .filter(|o| matches!(o.result, TestResult::Pass { .. }))
        .count();
    let failed = outcomes
        .iter()
        .filter(|o| matches!(o.result, TestResult::Fail { .. }))
        .count();
    let errored = outcomes
        .iter()
        .filter(|o| matches!(o.result, TestResult::Error(_)))
        .count();

    tracing::info!("{}", "-".repeat(70));
    tracing::info!(
        "Results: {} passed, {} failed, {} errors  (total {})",
        passed,
        failed,
        errored,
        outcomes.len()
    );

    let report_path = generate_report(&outcomes, &workspace_templates_dir())?;
    tracing::info!("Report written to: {}", report_path.display());

    Ok(failed == 0 && errored == 0)
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
