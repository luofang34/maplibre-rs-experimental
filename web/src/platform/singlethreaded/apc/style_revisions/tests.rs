#![allow(clippy::expect_used, clippy::panic)]

use std::{cell::RefCell, sync::Arc};

use maplibre::style::Style;
use wasm_bindgen_test::wasm_bindgen_test;

use super::{StyleRevisionError, StyleRevisions};

fn recording(
    posts: &RefCell<Vec<u64>>,
) -> impl FnOnce(u64, &str) -> Result<(), std::io::Error> + '_ {
    |revision, json| {
        assert!(json.contains("\"version\""), "the whole style is sent");
        posts.borrow_mut().push(revision);
        Ok(())
    }
}

#[wasm_bindgen_test]
fn a_style_is_sent_once_however_many_calls_name_it() {
    let posts = RefCell::new(Vec::new());
    let mut revisions = StyleRevisions::default();
    let style = Arc::new(Style::default());
    let first = revisions.revision(&style, recording(&posts)).expect("sent");
    let again = revisions
        .revision(&style, recording(&posts))
        .expect("named");
    let copy = Arc::new(Style::clone(&style));
    let unchanged = revisions.revision(&copy, recording(&posts)).expect("named");
    assert_eq!([again, unchanged], [first, first]);
    assert_eq!(*posts.borrow(), [first], "only the first call sends it");
}

#[wasm_bindgen_test]
fn a_changed_style_is_sent_under_a_new_revision() {
    let posts = RefCell::new(Vec::new());
    let mut revisions = StyleRevisions::default();
    let first = revisions
        .revision(&Arc::new(Style::default()), recording(&posts))
        .expect("sent");
    let changed = Arc::new(Style {
        glyphs: Some("https://example.invalid/{fontstack}/{range}.pbf".into()),
        ..Style::default()
    });
    let second = revisions
        .revision(&changed, recording(&posts))
        .expect("sent");
    assert_ne!(first, second);
    assert_eq!(*posts.borrow(), [first, second]);
}

#[wasm_bindgen_test]
fn a_style_the_workers_did_not_get_is_sent_again() {
    let posts = RefCell::new(Vec::new());
    let mut revisions = StyleRevisions::default();
    let style = Arc::new(Style::default());
    let error = revisions
        .revision(&style, |_, _| Err(std::io::Error::other("worker gone")))
        .expect_err("not sent");
    assert!(matches!(error, StyleRevisionError::Post { .. }));
    let revision = revisions.revision(&style, recording(&posts)).expect("sent");
    assert_eq!(*posts.borrow(), [revision]);
}
