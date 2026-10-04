//! The crate declares the license expression the repository's license files back: the fork's
//! own work under AGPL-3.0-or-later, the upstream maplibre-rs code under MIT or Apache-2.0.

#![allow(clippy::expect_used)]

use std::{fs, path::Path};

#[test]
fn the_declared_license_matches_the_license_files() {
    assert_eq!(
        env!("CARGO_PKG_LICENSE"),
        "AGPL-3.0-or-later AND (MIT OR Apache-2.0)"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (file, title) in [
        ("LICENSE-AGPL", "GNU AFFERO GENERAL PUBLIC LICENSE"),
        ("LICENSE-MIT", "MIT License"),
        ("LICENSE-APACHE", "Apache License"),
    ] {
        let text = fs::read_to_string(root.join(file)).expect(file);
        assert!(text.contains(title), "{file} is the {title}");
    }
}
