#![allow(clippy::expect_used, clippy::panic)]

use indicate_instrument_state::{EstimateQuality, GroupId, NavSource};

use super::{CANONICAL_STATES, fully_fed, nav2_source, nothing_fed, source_unusable, typical};

#[test]
fn corpus_ids_are_unique_and_well_formed() {
    for (position, state) in CANONICAL_STATES.iter().enumerate() {
        assert!(
            state
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{} violates the id charset",
            state.id
        );
        assert!(
            !CANONICAL_STATES[..position]
                .iter()
                .any(|s| s.id == state.id),
            "{} repeats",
            state.id
        );
    }
}

#[test]
fn the_corpus_spans_the_intended_situations() {
    // Nothing fed: no group ever arrived.
    let cold = nothing_fed();
    assert!(cold.attitude.data.is_none());
    assert!(cold.monitor_text.data.is_none());
    // Typical: attitude present with declared trust.
    let cruise = typical();
    assert!(cruise.attitude.data.is_some());
    assert_eq!(cruise.quality, EstimateQuality::Good);
    // Fully fed: every optional group present at once, including the
    // monitor channel and both nav idents.
    let full = fully_fed();
    assert!(full.variation.data.is_some());
    let monitor = full.monitor_text.data.expect("monitor channel present");
    assert_eq!(monitor.lines().len(), 2);
    let nav = full.nav.data.expect("nav present");
    assert_eq!(nav.to_ident.as_str(), "KMRY");
    assert_eq!(nav.from_ident.as_str(), "WPT-2");
    // Source unusable: values present, trust says do not use.
    let unusable = source_unusable();
    assert!(unusable.attitude.data.is_some());
    assert_eq!(unusable.quality, EstimateQuality::Unusable);
    // Nav2 selected: every nav source the HSI labels is one the corpus
    // names — GPS in `typical`, Nav1 in `fully-fed`, Nav2 here.
    let second = nav2_source();
    assert_eq!(
        second.nav.data.expect("nav present").source,
        NavSource::Nav2
    );
}

#[test]
fn withholding_is_expressible_for_every_corpus_state() {
    // The admission harness withholds groups one at a time; the corpus
    // must survive that lever without panicking for every group.
    for state in CANONICAL_STATES {
        for group in GroupId::ALL {
            let built = (state.build)();
            let _withheld = indicate_instrument_state::withhold_group(&built, group);
        }
    }
}
