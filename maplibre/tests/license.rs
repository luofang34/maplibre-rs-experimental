//! Every crate and package declares the license expression the repository's license files
//! back: the fork's own work under AGPL-3.0-or-later, and the upstream maplibre-rs code under
//! the licenses upstream declared for it, MIT or Apache-2.0 for crates and MIT for packages.

#![allow(clippy::expect_used)]

use std::{fs, path::Path};

const MIXED_CRATE: &str = "AGPL-3.0-or-later AND (MIT OR Apache-2.0)";
const MIXED_PACKAGE: &str = "AGPL-3.0-or-later AND MIT";
const FORK_ONLY: &str = "AGPL-3.0-or-later";

fn read(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    fs::read_to_string(root.join(path)).expect(path)
}

/// The value of the manifest's `license = "…"` line, if it has one.
fn cargo_license(manifest: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let value = line.trim().strip_prefix("license")?.trim_start();
        let value = value.strip_prefix('=')?.trim();
        Some(value.trim_matches('"').to_owned())
    })
}

#[test]
fn the_license_files_are_complete() {
    let agpl = read("LICENSE-AGPL");
    assert!(agpl.starts_with("                    GNU AFFERO GENERAL PUBLIC LICENSE"));
    assert!(agpl.ends_with("<https://www.gnu.org/licenses/>.\n"));
    assert_eq!(agpl.len(), 34523, "the verbatim text from gnu.org");
    assert!(read("LICENSE-MIT").starts_with("MIT License"));
    assert!(read("LICENSE-APACHE").contains("Apache License"));
}

#[test]
fn every_crate_declares_the_licenses_of_its_code() {
    assert_eq!(env!("CARGO_PKG_LICENSE"), MIXED_CRATE);
    let workspace = read("Cargo.toml");
    assert_eq!(cargo_license(&workspace).as_deref(), Some(MIXED_CRATE));
    let members = workspace
        .split_once("members = [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .expect("workspace members")
        .0;
    for member in members
        .split(',')
        .map(|member| member.trim().trim_matches('"'))
    {
        if member.is_empty() {
            continue;
        }
        let manifest = read(&format!("{member}/Cargo.toml"));
        assert!(
            manifest.contains("license.workspace = true"),
            "{member} inherits the workspace license"
        );
    }
    for crate_only_in_fork in [
        "apple/visionos/Cargo.toml",
        "apple/visionos/MapLibreVision/IndicateOverlay/bridge/Cargo.toml",
    ] {
        assert_eq!(
            cargo_license(&read(crate_only_in_fork)).as_deref(),
            Some(FORK_ONLY),
            "{crate_only_in_fork}"
        );
    }
}

#[test]
fn every_package_declares_the_licenses_of_its_code() {
    for (package, license) in [
        ("web/lib", MIXED_PACKAGE),
        ("web/demo", MIXED_PACKAGE),
        ("web/xr", FORK_ONLY),
    ] {
        for file in ["package.json", "package-lock.json"] {
            let path = format!("{package}/{file}");
            let json: serde_json::Value = serde_json::from_str(&read(&path)).expect(&path);
            let declared = if file == "package.json" {
                &json["license"]
            } else {
                &json["packages"][""]["license"]
            };
            assert_eq!(declared, license, "{path}");
        }
    }
}
