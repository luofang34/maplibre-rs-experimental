use std::{collections::BTreeMap, path::PathBuf};

use super::{
    build_updated_ignores, evaluate_ci, format_markdown_summary, parse_cli_args, IgnoreFile,
};
use crate::{comparison::Difference, CaseKind, TestOutcome, TestResult};

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
