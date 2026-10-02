#![allow(clippy::expect_used, clippy::panic)]

use std::string::String;
use std::vec;

use indicate_instrument_descriptor::{PanelDescriptor, PanelSet};
use indicate_instrument_registry::{Registry, scene_digest};
use indicate_instrument_scene::MAX_SCENE_BYTES;

use super::{
    AUTOFLIGHT_DESCRIPTOR, BUILTIN_PANELS, HSI_DESCRIPTOR, MONITOR_DESCRIPTOR, PFD_DESCRIPTOR,
};

fn hex(digest: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn digest_of(panels: &'static [PanelDescriptor]) -> String {
    let registry = Registry::new(panels).expect("composes");
    let mut scratch = vec![0u8; MAX_SCENE_BYTES];
    hex(scene_digest(&registry, &mut scratch).expect("digests"))
}

#[test]
fn the_builtin_scene_digest_is_pinned() {
    assert_eq!(digest_of(BUILTIN_PANELS), super::BUILTIN_SCENE_DIGEST);
}

#[test]
fn the_digest_moves_when_the_composition_moves() {
    static PFD_ONLY: [PanelDescriptor; 1] = [PFD_DESCRIPTOR];
    static REVERSED: [PanelDescriptor; 2] = [HSI_DESCRIPTOR, PFD_DESCRIPTOR];
    let full = digest_of(BUILTIN_PANELS);
    assert_ne!(digest_of(&PFD_ONLY), full, "dropping a panel must move it");
    assert_ne!(digest_of(&REVERSED), full, "panel order is contractual");
}

/// The mechanism #6 asks for is a pure refactor of composition, not of
/// paint: naming the shipped panels as a set must reproduce the pinned
/// digest byte for byte, or set identity has leaked into cross-shell
/// identity.
#[test]
fn composing_the_builtin_set_reproduces_the_pinned_digest() {
    static SETS: [&PanelSet; 1] = [&super::BUILTIN_SET];
    let registry = Registry::from_sets(&SETS).expect("the shipped set composes");
    let mut scratch = vec![0u8; MAX_SCENE_BYTES];
    assert_eq!(
        hex(scene_digest(&registry, &mut scratch).expect("digests")),
        super::BUILTIN_SCENE_DIGEST,
    );
}

/// Sets are packaging, not paint: the same panels in the same order
/// digest the same however they are grouped.
#[test]
fn regrouping_panels_without_reordering_them_is_digest_neutral() {
    static HEAD: PanelSet = PanelSet {
        id: "head",
        panels: &[PFD_DESCRIPTOR],
    };
    static TAIL: PanelSet = PanelSet {
        id: "tail",
        panels: &[HSI_DESCRIPTOR, AUTOFLIGHT_DESCRIPTOR, MONITOR_DESCRIPTOR],
    };
    static SPLIT: [&PanelSet; 2] = [&HEAD, &TAIL];
    let registry = Registry::from_sets(&SPLIT).expect("composes");
    let mut scratch = vec![0u8; MAX_SCENE_BYTES];
    assert_eq!(
        hex(scene_digest(&registry, &mut scratch).expect("digests")),
        super::BUILTIN_SCENE_DIGEST,
        "set boundaries must not reach the digest",
    );
}
