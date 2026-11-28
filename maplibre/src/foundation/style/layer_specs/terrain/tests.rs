#![allow(clippy::expect_used, clippy::panic)]

use super::TerrainSpecification;

#[test]
fn exaggeration_defaults_to_one() {
    let terrain: TerrainSpecification =
        serde_json::from_str(r#"{"source": "dem"}"#).expect("terrain parses");

    assert_eq!(terrain.source, "dem");
    assert_eq!(terrain.exaggeration, 1.0);
}

#[test]
fn explicit_exaggeration_is_kept() {
    let terrain: TerrainSpecification =
        serde_json::from_str(r#"{"source": "dem", "exaggeration": 2}"#).expect("terrain parses");

    assert_eq!(terrain.exaggeration, 2.0);
}
