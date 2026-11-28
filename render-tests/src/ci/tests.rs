use std::{collections::BTreeMap, path::PathBuf};

use super::{
    build_updated_ignores, evaluate_ci, evaluate_ci_for_options, format_markdown_summary,
    parse_cli_args, CliOptions, IgnoreFile,
};
use crate::{
    comparison::Difference,
    paths::{filter_tests, workspace_tests_dir},
    report::{copy_fixture_artifacts, ensure_artifact_dir, write_lifecycle_json, FixtureTelemetry},
    runner::render_case::FrameCleanupGuard,
    CaseKind, TestOutcome, TestResult,
};

fn sample_outcome(id: &str, kind: CaseKind, result: TestResult) -> TestOutcome {
    TestOutcome {
        id: id.to_owned(),
        kind,
        result,
        attempts: 1,
    }
}

fn zero_diff() -> Difference {
    Difference {
        mean: 0.0,
        solid: 0,
    }
}

fn high_diff() -> Difference {
    Difference {
        mean: 0.25,
        solid: 100,
    }
}

#[test]
fn cli_parser_handles_ci_flags_and_custom_root() {
    let parsed = parse_cli_args([
        "--ci",
        "--ignores",
        "custom-ignores.json",
        "--summary-json",
        "out.json",
        "--summary-md",
        "out.md",
        "render-tests/src/tests/query",
    ])
    .expect("valid flags");
    assert!(parsed.ci);
    assert!(!parsed.write_ignores);
    assert_eq!(parsed.ignores_path, PathBuf::from("custom-ignores.json"));
    assert_eq!(parsed.summary_json, Some(PathBuf::from("out.json")));
    assert_eq!(parsed.summary_md, Some(PathBuf::from("out.md")));
    assert_eq!(
        parsed.test_root,
        PathBuf::from("render-tests/src/tests/query")
    );
    assert!(parse_cli_args(["--unknown"]).is_err());
    assert!(parse_cli_args(["--ignores"]).is_err());
}

#[test]
fn ratchet_passes_when_only_listed_cases_fail_and_none_unexpectedly_pass() {
    let outcomes = vec![
        sample_outcome(
            "fill-color/default",
            CaseKind::Render,
            TestResult::Pass(zero_diff()),
        ),
        sample_outcome(
            "canvas/default",
            CaseKind::Render,
            TestResult::Error("unknown variant `canvas`".into()),
        ),
        sample_outcome(
            "query/basic",
            CaseKind::Query,
            TestResult::Pass(zero_diff()),
        ),
        sample_outcome(
            "query/symbol/filtered",
            CaseKind::Query,
            TestResult::Fail(high_diff()),
        ),
    ];
    let mut ignores = BTreeMap::new();
    ignores.insert(
        "canvas/default".to_owned(),
        "Canvas source unsupported".to_owned(),
    );
    ignores.insert(
        "query/symbol/filtered".to_owned(),
        "Symbol filter query mismatch".to_owned(),
    );

    let summary = evaluate_ci(&outcomes, &ignores, true);
    assert!(summary.ci_passed());
    assert_eq!(summary.total.total, 4);
    assert_eq!(summary.total.passed, 2);
    assert_eq!(summary.total.ignored, 2);
    assert_eq!(summary.render.passed, 1);
    assert_eq!(summary.query.passed, 1);

    let markdown = format_markdown_summary(&summary);
    assert!(markdown.contains("Render (`expected.png`) | 1 | 1 | 0 | 2 | 50.00%"));
    assert!(markdown.contains("Query (`expected.json`) | 1 | 1 | 0 | 2 | 50.00%"));
}

#[test]
fn ratchet_fails_on_unlisted_failure_unexpected_pass_or_stale_entry() {
    let outcomes = vec![
        sample_outcome(
            "fill-color/default",
            CaseKind::Render,
            TestResult::Fail(high_diff()),
        ),
        sample_outcome(
            "canvas/default",
            CaseKind::Render,
            TestResult::Pass(zero_diff()),
        ),
    ];
    let mut ignores = BTreeMap::new();
    ignores.insert(
        "canvas/default".to_owned(),
        "Canvas source unsupported".to_owned(),
    );
    ignores.insert("deleted/fixture".to_owned(), "Removed fixture".to_owned());

    let summary = evaluate_ci(&outcomes, &ignores, true);
    assert!(!summary.ci_passed());
    assert_eq!(summary.unexpected_failures.len(), 1);
    assert_eq!(summary.unexpected_passes, vec!["canvas/default".to_owned()]);
    assert_eq!(summary.stale_ignores, vec!["deleted/fixture".to_owned()]);
}

#[test]
fn write_ignores_preserves_custom_reasons_and_rejects_blank_reasons() {
    let mut existing = IgnoreFile::default();
    existing.ignores.insert(
        "canvas/default".to_owned(),
        "Custom hand-written canvas explanation".to_owned(),
    );
    existing
        .ignores
        .insert("now-passing/case".to_owned(), "Used to fail".to_owned());

    let outcomes = vec![
        sample_outcome(
            "now-passing/case",
            CaseKind::Render,
            TestResult::Pass(zero_diff()),
        ),
        sample_outcome(
            "canvas/default",
            CaseKind::Render,
            TestResult::Error("unknown variant `canvas`".into()),
        ),
        sample_outcome(
            "video/default",
            CaseKind::Render,
            TestResult::Error("unknown variant `video`".into()),
        ),
    ];
    let updated = build_updated_ignores(&outcomes, Some(&existing));
    assert_eq!(updated.ignores.len(), 2);
    assert_eq!(
        updated.ignores.get("canvas/default").map(String::as_str),
        Some("Custom hand-written canvas explanation")
    );
    assert_eq!(
        updated.ignores.get("video/default").map(String::as_str),
        Some("Video source (`type: \"video\"`) is not implemented")
    );
    assert!(!updated.ignores.contains_key("now-passing/case"));

    let dir = tempfile::tempdir().expect("tempdir");
    let bad_path = dir.path().join("bad.json");
    let mut bad = updated;
    bad.ignores
        .insert("empty/reason".to_owned(), "   ".to_owned());
    assert!(bad.save(&bad_path).is_err());
}

#[test]
fn checked_in_ignores_json_loads_and_matches_existing_fixtures() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ignores_path = manifest_dir.join("ignores.json");
    let tests_root = manifest_dir.join("src/tests");
    let file = IgnoreFile::load(&ignores_path).expect("valid render-tests/ignores.json");
    assert_eq!(file.baseline.ignored, file.ignores.len());
    for id in file.ignores.keys() {
        let style_path = tests_root.join(id).join("style.json");
        assert!(
            style_path.is_file(),
            "ignored fixture `{id}` does not exist at {}",
            style_path.display()
        );
    }
}

#[test]
fn cli_parser_handles_filter_lifecycle_and_artifact_flags() {
    let parsed = parse_cli_args([
        "--filter",
        "basic-v9/",
        "--filter",
        "query/",
        "--lifecycle-json",
        "out/lifecycle.json",
        "--artifact-dir",
        "out/artifacts",
    ])
    .expect("valid flags");
    assert_eq!(
        parsed.filters,
        vec!["basic-v9/".to_owned(), "query/".to_owned()]
    );
    assert_eq!(
        parsed.lifecycle_json,
        Some(PathBuf::from("out/lifecycle.json"))
    );
    assert_eq!(parsed.artifact_dir, Some(PathBuf::from("out/artifacts")));
    assert!(parse_cli_args(["--filter"]).is_err());
    assert!(parse_cli_args(["--lifecycle-json"]).is_err());
    assert!(parse_cli_args(["--artifact-dir"]).is_err());
}

#[test]
fn filter_tests_matches_prefixes_and_fails_fast_when_empty() {
    let root = tempfile::tempdir().expect("tempdir");
    let first = root.path().join("basic-v9/z0");
    let second = root.path().join("basic-v9/z1");
    let third = root.path().join("query/point");
    let kept = filter_tests(
        vec![first.clone(), second.clone(), third.clone()],
        root.path(),
        &["basic-v9/".to_owned()],
    )
    .expect("matching prefix");
    assert_eq!(kept, vec![first.clone(), second.clone()]);

    let error = filter_tests(
        vec![first, second, third],
        root.path(),
        &["missing/".to_owned()],
    )
    .expect_err("empty");
    assert!(error.contains("No tests matched --filter"), "{error}");
}

#[test]
fn evaluate_ci_skips_stale_ignores_when_filter_is_non_empty() {
    let outcomes = vec![sample_outcome(
        "fill-color/default",
        CaseKind::Render,
        TestResult::Pass(zero_diff()),
    )];
    let mut ignores = BTreeMap::new();
    ignores.insert(
        "deleted/fixture".to_owned(),
        "Unvisited under filter".to_owned(),
    );

    let filtered_options = CliOptions {
        test_root: workspace_tests_dir(),
        ci: true,
        filters: vec!["fill-color/".to_owned()],
        ..CliOptions::default()
    };
    let filtered = evaluate_ci_for_options(&outcomes, &ignores, &filtered_options);
    assert!(filtered.stale_ignores.is_empty());
    assert!(filtered.ci_passed());

    let full_options = CliOptions {
        test_root: workspace_tests_dir(),
        ci: true,
        ..CliOptions::default()
    };
    let full = evaluate_ci_for_options(&outcomes, &ignores, &full_options);
    assert_eq!(full.stale_ignores, vec!["deleted/fixture".to_owned()]);
    assert!(!full.ci_passed());
}

#[test]
fn lifecycle_json_is_sorted_and_contains_telemetry_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output_path = dir.path().join("nested/lifecycle.json");
    let mut second = FixtureTelemetry::fallback("query/b".to_owned(), CaseKind::Query, true);
    second.passed = true;
    let first = FixtureTelemetry {
        id: "basic-v9/a".to_owned(),
        kind: "render",
        center: Some([11.5, 48.1]),
        zoom: Some(12.0),
        pitch: Some(30.0),
        bearing: Some(45.0),
        fov_deg: Some(36.87),
        center_elevation_m: Some(520.0),
        required_tile_count: 4,
        dem_tile_count: 2,
        paused_source_count: 1,
        frame_count: 24,
        draws: 18,
        state_changes: 9,
        mean_diff: Some(0.001),
        solid_diff: Some(0),
        passed: true,
        ignored: false,
    };

    write_lifecycle_json(&output_path, &[second, first]).expect("write lifecycle json");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&output_path).expect("read lifecycle json"))
            .expect("valid json");
    let fixtures = parsed
        .get("fixtures")
        .and_then(serde_json::Value::as_array)
        .expect("fixtures array");
    let first_entry = fixtures.first().expect("first fixture");
    let second_entry = fixtures.get(1).expect("second fixture");
    assert_eq!(
        first_entry.get("id").and_then(serde_json::Value::as_str),
        Some("basic-v9/a")
    );
    assert_eq!(
        second_entry.get("id").and_then(serde_json::Value::as_str),
        Some("query/b")
    );
    assert_eq!(
        first_entry
            .get("fov_deg")
            .and_then(serde_json::Value::as_f64),
        Some(36.87)
    );
    assert_eq!(
        first_entry
            .get("center_elevation_m")
            .and_then(serde_json::Value::as_f64),
        Some(520.0)
    );
    assert_eq!(
        first_entry.get("draws").and_then(serde_json::Value::as_u64),
        Some(18)
    );
    assert_eq!(
        second_entry
            .get("ignored")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[test]
fn artifact_dir_copies_render_outputs_with_slug_names_and_skips_queries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let render_dir = dir.path().join("tests/basic-v9/z0");
    let query_dir = dir.path().join("tests/query/point");
    let missing_dir = dir.path().join("tests/canvas/default");
    let artifact_dir = dir.path().join("artifacts/run-1");
    std::fs::create_dir_all(&render_dir).expect("render dir");
    std::fs::create_dir_all(&query_dir).expect("query dir");
    std::fs::create_dir_all(&missing_dir).expect("missing dir");
    std::fs::write(render_dir.join("actual.png"), b"actual-bytes").expect("actual");
    std::fs::write(render_dir.join("diff.png"), b"diff-bytes").expect("diff");
    std::fs::write(query_dir.join("actual.png"), b"should-not-copy").expect("query png");

    ensure_artifact_dir(&artifact_dir).expect("create artifact dir");
    copy_fixture_artifacts(&artifact_dir, &render_dir, "basic-v9/z0", CaseKind::Render)
        .expect("copy render artifacts");
    copy_fixture_artifacts(&artifact_dir, &query_dir, "query/point", CaseKind::Query)
        .expect("skip query artifacts");
    copy_fixture_artifacts(
        &artifact_dir,
        &missing_dir,
        "canvas/default",
        CaseKind::Render,
    )
    .expect("tolerate errored render without pngs");

    assert_eq!(
        std::fs::read(artifact_dir.join("basic-v9__z0-actual.png")).expect("copied actual"),
        b"actual-bytes"
    );
    assert_eq!(
        std::fs::read(artifact_dir.join("basic-v9__z0-diff.png")).expect("copied diff"),
        b"diff-bytes"
    );
    assert!(!artifact_dir.join("query__point-actual.png").exists());
}

#[test]
#[allow(clippy::panic)]
fn frame_cleanup_guard_removes_frames_on_early_drop_and_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths: Vec<PathBuf> = (0..=2)
        .map(|idx| dir.path().join(format!("frame_{idx}.png")))
        .collect();
    for path in &paths {
        std::fs::write(path, b"frame").expect("create frame");
    }

    let early_result: Result<(), &str> = {
        let _guard = FrameCleanupGuard::new(paths.clone());
        Err("simulated failure")
    };
    assert!(early_result.is_err());
    assert!(paths.iter().all(|path| !path.exists()));

    for path in &paths {
        std::fs::write(path, b"frame").expect("recreate frame");
    }
    let panic_result = std::panic::catch_unwind(|| {
        let _guard = FrameCleanupGuard::new(paths.clone());
        panic!("simulated panic");
    });
    assert!(panic_result.is_err());
    assert!(paths.iter().all(|path| !path.exists()));
}
