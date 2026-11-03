#![allow(clippy::expect_used)]

use std::sync::Arc;

use maplibre::style::Style;
use wasm_bindgen_test::wasm_bindgen_test;

use super::{WorkerStyles, KEPT};

#[wasm_bindgen_test]
fn every_call_of_a_revision_shares_its_style() {
    let mut styles = WorkerStyles::default();
    styles.insert(3, Style::default());
    let first = styles.get(3).expect("kept");
    let second = styles.get(3).expect("kept");
    assert!(Arc::ptr_eq(&first, &second));
    assert!(styles.get(4).is_none());
}

#[wasm_bindgen_test]
fn only_recent_revisions_are_kept() {
    let mut styles = WorkerStyles::default();
    for revision in 0..=KEPT as u64 {
        styles.insert(revision, Style::default());
    }
    assert!(styles.get(0).is_none(), "the oldest is forgotten");
    assert!(styles.get(1).is_some());
    assert!(styles.get(KEPT as u64).is_some());
}
