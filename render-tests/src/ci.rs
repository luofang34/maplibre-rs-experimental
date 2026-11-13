//! CI ignore-list ratcheting, CLI option parsing, and summary emission for `render-tests`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    paths::{workspace_ignores_path, workspace_tests_dir},
    CaseKind, TestOutcome, TestResult,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CliOptions {
    pub(super) test_root: PathBuf,
    pub(super) ci: bool,
    pub(super) write_ignores: bool,
    pub(super) ignores_path: PathBuf,
    pub(super) summary_json: Option<PathBuf>,
    pub(super) summary_md: Option<PathBuf>,
}

impl Default for CliOptions {
    fn default() -> Self {
        Self {
            test_root: workspace_tests_dir(),
            ci: false,
            write_ignores: false,
            ignores_path: workspace_ignores_path(),
            summary_json: None,
            summary_md: None,
        }
    }
}

pub(super) fn parse_cli_args<I, S>(args: I) -> Result<CliOptions, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut options = CliOptions::default();
    let mut positional_root: Option<PathBuf> = None;
    let mut iter = args.into_iter().map(|arg| arg.as_ref().to_owned());

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ci" => options.ci = true,
            "--write-ignores" => options.write_ignores = true,
            "--ignores" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing path after --ignores".to_owned())?;
                options.ignores_path = PathBuf::from(value);
            }
            "--summary-json" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing path after --summary-json".to_owned())?;
                options.summary_json = Some(PathBuf::from(value));
            }
            "--summary-md" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "Missing path after --summary-md".to_owned())?;
                options.summary_md = Some(PathBuf::from(value));
            }
            flag if flag.starts_with('-') => {
                return Err(format!(
                    "Unknown flag `{flag}`. Supported: --ci, --write-ignores, --ignores <path>, --summary-json <path>, --summary-md <path> [test-dir]"
                ));
            }
            path => {
                if positional_root.is_some() {
                    return Err(format!("Unexpected second test directory `{path}`"));
                }
                positional_root = Some(PathBuf::from(path));
            }
        }
    }

    if let Some(root) = positional_root {
        options.test_root = root;
    }
    Ok(options)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct BaselineMetadata {
    pub(super) total: usize,
    pub(super) passed: usize,
    pub(super) ignored: usize,
    pub(super) render_total: usize,
    pub(super) render_passed: usize,
    pub(super) query_total: usize,
    pub(super) query_passed: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct IgnoreFile {
    pub(super) baseline: BaselineMetadata,
    pub(super) ignores: BTreeMap<String, String>,
}

impl IgnoreFile {
    pub(super) fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|err| format!("Cannot read ignores file {}: {err}", path.display()))?;
        let parsed: Self = serde_json::from_str(&raw)
            .map_err(|err| format!("Cannot parse ignores file {}: {err}", path.display()))?;
        parsed.validate(path)?;
        Ok(parsed)
    }

    pub(super) fn validate(&self, path: &Path) -> Result<(), String> {
        for (id, reason) in &self.ignores {
            if id.trim().is_empty() {
                return Err(format!("Empty fixture ID in {}", path.display()));
            }
            if reason.trim().is_empty() {
                return Err(format!(
                    "Fixture `{id}` in {} has an empty ignore reason",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    pub(super) fn save(&self, path: &Path) -> Result<(), String> {
        self.validate(path)?;
        let mut formatted = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Cannot serialize ignores file: {err}"))?;
        formatted.push('\n');
        std::fs::write(path, formatted)
            .map_err(|err| format!("Cannot write {}: {err}", path.display()))
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub(super) struct CategorySummary {
    pub(super) total: usize,
    pub(super) passed: usize,
    pub(super) ignored: usize,
    pub(super) unexpected_failures: usize,
    pub(super) pass_rate_percent: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub(super) struct SuiteSummary {
    pub(super) total: CategorySummary,
    pub(super) render: CategorySummary,
    pub(super) query: CategorySummary,
    pub(super) unexpected_failures: Vec<String>,
    pub(super) unexpected_passes: Vec<String>,
    pub(super) stale_ignores: Vec<String>,
}

impl SuiteSummary {
    pub(super) fn ci_passed(&self) -> bool {
        self.unexpected_failures.is_empty()
            && self.unexpected_passes.is_empty()
            && self.stale_ignores.is_empty()
    }
}

fn rate(passed: usize, total: usize) -> f64 {
    if total == 0 {
        100.0
    } else {
        (passed as f64 * 10_000.0 / total as f64).round() / 100.0
    }
}

pub(super) fn evaluate_ci(
    outcomes: &[TestOutcome],
    ignores: &BTreeMap<String, String>,
    check_stale_ignores: bool,
) -> SuiteSummary {
    let mut render_total = 0_usize;
    let mut render_passed = 0_usize;
    let mut render_ignored = 0_usize;
    let mut render_unexpected_fail = 0_usize;

    let mut query_total = 0_usize;
    let mut query_passed = 0_usize;
    let mut query_ignored = 0_usize;
    let mut query_unexpected_fail = 0_usize;

    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut seen_ids = BTreeSet::new();

    for outcome in outcomes {
        seen_ids.insert(outcome.id.as_str());
        let is_ignored = ignores.contains_key(&outcome.id);
        let passed = matches!(outcome.result, TestResult::Pass(_));

        match outcome.kind {
            CaseKind::Render => render_total += 1,
            CaseKind::Query => query_total += 1,
        }

        match (passed, is_ignored) {
            (true, false) => match outcome.kind {
                CaseKind::Render => render_passed += 1,
                CaseKind::Query => query_passed += 1,
            },
            (true, true) => {
                match outcome.kind {
                    CaseKind::Render => render_passed += 1,
                    CaseKind::Query => query_passed += 1,
                }
                unexpected_passes.push(outcome.id.clone());
            }
            (false, true) => match outcome.kind {
                CaseKind::Render => render_ignored += 1,
                CaseKind::Query => query_ignored += 1,
            },
            (false, false) => {
                match outcome.kind {
                    CaseKind::Render => render_unexpected_fail += 1,
                    CaseKind::Query => query_unexpected_fail += 1,
                }
                let detail = match &outcome.result {
                    TestResult::Fail(diff) => format!("{} ({diff})", outcome.id),
                    TestResult::Error(msg) => format!("{}: {msg}", outcome.id),
                    TestResult::Pass(_) => outcome.id.clone(),
                };
                unexpected_failures.push(detail);
            }
        }
    }

    let stale_ignores = if check_stale_ignores {
        ignores
            .keys()
            .filter(|id| !seen_ids.contains(id.as_str()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let total_cases = render_total + query_total;
    let total_passed = render_passed + query_passed;
    let total_ignored = render_ignored + query_ignored;
    let total_unexpected_fail = render_unexpected_fail + query_unexpected_fail;

    SuiteSummary {
        total: CategorySummary {
            total: total_cases,
            passed: total_passed,
            ignored: total_ignored,
            unexpected_failures: total_unexpected_fail,
            pass_rate_percent: rate(total_passed, total_cases),
        },
        render: CategorySummary {
            total: render_total,
            passed: render_passed,
            ignored: render_ignored,
            unexpected_failures: render_unexpected_fail,
            pass_rate_percent: rate(render_passed, render_total),
        },
        query: CategorySummary {
            total: query_total,
            passed: query_passed,
            ignored: query_ignored,
            unexpected_failures: query_unexpected_fail,
            pass_rate_percent: rate(query_passed, query_total),
        },
        unexpected_failures,
        unexpected_passes,
        stale_ignores,
    }
}

pub(super) fn format_markdown_summary(summary: &SuiteSummary) -> String {
    let status = if summary.ci_passed() { "PASS" } else { "FAIL" };
    let mut md = format!(
        "## Render & Query Conformance (`{status}`)\n\n\
         | Suite | Passed | Ignored | Unexpected Fail | Total | Pass Rate |\n\
         | :--- | ---: | ---: | ---: | ---: | ---: |\n\
         | Render (`expected.png`) | {} | {} | {} | {} | {:.2}% |\n\
         | Query (`expected.json`) | {} | {} | {} | {} | {:.2}% |\n\
         | **Total** | **{}** | **{}** | **{}** | **{}** | **{:.2}%** |\n",
        summary.render.passed,
        summary.render.ignored,
        summary.render.unexpected_failures,
        summary.render.total,
        summary.render.pass_rate_percent,
        summary.query.passed,
        summary.query.ignored,
        summary.query.unexpected_failures,
        summary.query.total,
        summary.query.pass_rate_percent,
        summary.total.passed,
        summary.total.ignored,
        summary.total.unexpected_failures,
        summary.total.total,
        summary.total.pass_rate_percent,
    );
    if !summary.unexpected_failures.is_empty() {
        md.push_str("\n### Unexpected Failures\n");
        for item in &summary.unexpected_failures {
            md.push_str(&format!("- `{item}`\n"));
        }
    }
    if !summary.unexpected_passes.is_empty() {
        md.push_str("\n### Unexpected Passes (remove from `render-tests/ignores.json`)\n");
        for id in &summary.unexpected_passes {
            md.push_str(&format!("- `{id}`\n"));
        }
    }
    if !summary.stale_ignores.is_empty() {
        md.push_str("\n### Stale Ignore Entries (missing fixture directory)\n");
        for id in &summary.stale_ignores {
            md.push_str(&format!("- `{id}`\n"));
        }
    }
    md
}

pub(super) fn write_summaries(
    summary: &SuiteSummary,
    json_path: Option<&Path>,
    md_path: Option<&Path>,
) -> Result<(), String> {
    if let Some(path) = json_path {
        let mut json = serde_json::to_string_pretty(summary)
            .map_err(|err| format!("Cannot serialize summary JSON: {err}"))?;
        json.push('\n');
        std::fs::write(path, json)
            .map_err(|err| format!("Cannot write {}: {err}", path.display()))?;
    }
    if let Some(path) = md_path {
        let md = format_markdown_summary(summary);
        std::fs::write(path, md)
            .map_err(|err| format!("Cannot write {}: {err}", path.display()))?;
    }
    Ok(())
}

pub(super) fn build_updated_ignores(
    outcomes: &[TestOutcome],
    existing: Option<&IgnoreFile>,
) -> IgnoreFile {
    let mut ignores = BTreeMap::new();
    let mut render_total = 0_usize;
    let mut render_passed = 0_usize;
    let mut query_total = 0_usize;
    let mut query_passed = 0_usize;

    for outcome in outcomes {
        match outcome.kind {
            CaseKind::Render => render_total += 1,
            CaseKind::Query => query_total += 1,
        }
        match &outcome.result {
            TestResult::Pass(_) => match outcome.kind {
                CaseKind::Render => render_passed += 1,
                CaseKind::Query => query_passed += 1,
            },
            failure => {
                let reason = existing
                    .and_then(|file| file.ignores.get(&outcome.id))
                    .filter(|text| !text.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| categorize_failure(&outcome.id, failure));
                ignores.insert(outcome.id.clone(), reason);
            }
        }
    }

    IgnoreFile {
        baseline: BaselineMetadata {
            total: render_total + query_total,
            passed: render_passed + query_passed,
            ignored: ignores.len(),
            render_total,
            render_passed,
            query_total,
            query_passed,
        },
        ignores,
    }
}

fn categorize_failure(id: &str, result: &TestResult) -> String {
    if let TestResult::Error(msg) = result {
        if msg.contains("unknown variant `canvas`") {
            return "Canvas source (`type: \"canvas\"`) is not implemented".to_owned();
        }
        if msg.contains("unknown variant `video`") {
            return "Video source (`type: \"video\"`) is not implemented".to_owned();
        }
        if msg.contains("addCustomLayer") {
            return "Style-level JS `addCustomLayer` operation is not supported".to_owned();
        }
        if msg.contains("querySourceFeatures") {
            return "`querySourceFeatures` harness operation is not implemented".to_owned();
        }
        if msg.contains("larger than the GPU limit of 8192") {
            return "Physical render target exceeds the guaranteed 8192 px WebGPU texture limit"
                .to_owned();
        }
        if msg.contains("render-tests/src/assets/styles/") {
            return "External `local://styles/*.json` basemap asset is not vendored".to_owned();
        }
        if msg.contains("has no tile template") {
            return "Remote Mapbox TileJSON source URL is not vendored offline".to_owned();
        }
        return format!("Harness or engine error: {msg}");
    }
    if id.starts_with("query/") {
        return "`queryRenderedFeatures` hit-testing differs from GL JS under rotation, tilt, viewport translation, or symbol filtering".to_owned();
    }
    if id.contains("atmosphere") || id.contains("fog-sky-blend") {
        return "GL JS `sky`/`atmosphere-blend` shader model differs from physical Rayleigh/Mie atmosphere rendering".to_owned();
    }
    if id.starts_with("text-font/") {
        return "CJK glyph fallback without local CJK font rasterization".to_owned();
    }
    format!("Pixel difference exceeds GL JS reference threshold on `{id}`")
}

#[cfg(test)]
mod tests;
