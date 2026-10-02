#![allow(clippy::expect_used, clippy::panic)]

use super::*;

/// A representative condition for every known identity.
const CATALOG: &[AlertCondition] = &[
    AlertCondition::Altitude(AltFault::ReferenceLost),
    AlertCondition::Altitude(AltFault::DatumMiscompare),
    AlertCondition::Altitude(AltFault::Unavailable),
    AlertCondition::Heading(NavFault::HeadingReferenceLost),
    AlertCondition::Heading(NavFault::CourseSourceInvalid),
    AlertCondition::Heading(NavFault::Unavailable),
    AlertCondition::TurnSlip(DynFault::TurnRateInvalid),
    AlertCondition::TurnSlip(DynFault::SlipInvalid),
    AlertCondition::TurnSlip(DynFault::Unavailable),
    AlertCondition::Miscompare(MiscompareFault::Attitude),
    AlertCondition::Miscompare(MiscompareFault::Airspeed),
    AlertCondition::Miscompare(MiscompareFault::Altitude),
    AlertCondition::Miscompare(MiscompareFault::Heading),
    AlertCondition::Display(DisplayFault::RendererStalled),
    AlertCondition::Display(DisplayFault::FrameGenerationLost),
    AlertCondition::Display(DisplayFault::CommandBufferCorrupt),
    AlertCondition::Display(DisplayFault::BackendLost),
    AlertCondition::Display(DisplayFault::RetainedImage),
    AlertCondition::FrameMismatch { code: 7 },
    AlertCondition::System(SystemNote::DatabaseStale),
    AlertCondition::System(SystemNote::MaintenanceRequired),
    AlertCondition::System(SystemNote::ConfigMismatch),
];

#[test]
fn identities_are_unique() {
    for (i, a) in CATALOG.iter().enumerate() {
        for b in &CATALOG[i + 1..] {
            assert_ne!(a.id(), b.id(), "collision between {a:?} and {b:?}");
        }
    }
}

#[test]
fn class_of_agrees_with_condition_class() {
    for cond in CATALOG {
        assert_eq!(
            class_of(cond.id()),
            Some(cond.class()),
            "class mismatch for {cond:?}"
        );
    }
}

#[test]
fn every_frame_code_resolves_to_caution() {
    for code in 0..=u8::MAX {
        let cond = AlertCondition::FrameMismatch { code };
        assert_eq!(cond.class(), AlertClass::Caution);
        assert_eq!(class_of(cond.id()), Some(AlertClass::Caution));
    }
}

#[test]
fn unknown_identities_resolve_to_none() {
    // Unused code inside a known family, and an unknown family.
    assert_eq!(class_of(AlertId(0x0109)), None);
    assert_eq!(class_of(AlertId(0x0900)), None);
}

#[test]
fn attitude_miscompare_is_a_warning() {
    assert_eq!(
        AlertCondition::Miscompare(MiscompareFault::Attitude).class(),
        AlertClass::Warning
    );
}

// ---- Display-reason registry (cross-shell contract) ----------------------

use std::format;
use std::string::String;
use std::vec::Vec;

/// The registry document the Swift and JavaScript mirrors track.
const REGISTRY_DOC: &str = include_str!("../../../../docs/instruments/display-reason-registry.md");

const REGISTRY_HEADING: &str = "## The registry";

const REGISTRY_HEADER: [&str; 5] = ["Code", "Reason", "Identity", "Class", "Meaning"];

/// The pinned append-only registry: variant, wire code, packed identity.
/// New reasons are appended; entries are never reordered or renumbered.
const PINNED_REGISTRY: [(DisplayFault, u8, u16); 5] = [
    (DisplayFault::RendererStalled, 1, 0x0501),
    (DisplayFault::FrameGenerationLost, 2, 0x0502),
    (DisplayFault::CommandBufferCorrupt, 3, 0x0503),
    (DisplayFault::BackendLost, 4, 0x0504),
    (DisplayFault::RetainedImage, 5, 0x0505),
];

#[test]
fn display_reason_codes_are_pinned() {
    assert_eq!(
        DisplayFault::ALL,
        PINNED_REGISTRY.map(|(fault, _, _)| fault),
        "registry entries are appended in ascending code order, never reordered"
    );
    for (fault, code, identity) in PINNED_REGISTRY {
        assert_eq!(fault.code(), code, "{fault:?} keeps its wire code");
        assert_eq!(DisplayFault::from_code(code), Some(fault));
        assert_eq!(AlertCondition::Display(fault).id(), AlertId(identity));
    }
}

#[test]
fn display_reason_decoding_is_fail_closed() {
    assert_eq!(DisplayFault::from_code(0), None);
    for code in 6..=u8::MAX {
        assert_eq!(DisplayFault::from_code(code), None);
        assert_eq!(class_of(AlertId(0x0500 | u16::from(code))), None);
    }
}

/// The registry section alone, bounded by the next heading, so a table
/// elsewhere in the document can never be read as a registry row.
fn registry_section() -> &'static str {
    let after = REGISTRY_DOC
        .split_once(REGISTRY_HEADING)
        .expect("the registry document still has a registry section")
        .1;
    after
        .split_once("\n## ")
        .map_or(after, |(section, _)| section)
}

fn registry_cells(row: &str) -> Vec<&str> {
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
    assert_eq!(
        cells.len(),
        REGISTRY_HEADER.len(),
        "a registry row must carry code, reason, identity, class, meaning: {row}"
    );
    cells
}

/// Lines of the registry section that belong to a markdown table.
fn registry_table_lines() -> Vec<&'static str> {
    registry_section()
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .collect()
}

/// The table's data rows, parsed positionally so a row cannot exempt
/// itself from the comparison by imitating the header.
fn documented_rows() -> Vec<Vec<String>> {
    let mut lines = registry_table_lines().into_iter();
    let header = lines
        .next()
        .expect("the registry section still contains a table");
    assert_eq!(
        registry_cells(header),
        REGISTRY_HEADER.to_vec(),
        "the registry table's columns must stay code, reason, identity, class, meaning"
    );
    let delimiter = lines.next().expect("the table still has a delimiter row");
    assert!(
        registry_cells(delimiter)
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':')),
        "the row under the header must be the table delimiter: {delimiter}"
    );
    lines
        .map(|line| registry_cells(line).into_iter().map(String::from).collect())
        .collect()
}

/// The row a reason implies: code, debug name, packed identity, class.
/// The meaning cell is prose and is checked only for presence.
fn expected_cells(fault: DisplayFault) -> [String; 4] {
    let id = AlertCondition::Display(fault).id();
    let class = class_of(id).expect("a registered reason has a class");
    [
        format!("{}", fault.code()),
        format!("`{fault:?}`"),
        format!("`0x{:04x}`", id.0),
        format!("{class:?}"),
    ]
}

#[test]
fn the_registry_section_holds_exactly_one_table() {
    let expected = DisplayFault::ALL.len() + 2;
    assert_eq!(
        registry_table_lines().len(),
        expected,
        "the registry section must hold one table: a header, a delimiter, \
         and one row per registered reason"
    );
}

#[test]
fn the_documented_registry_matches_the_code() {
    let rows = documented_rows();
    // Without this, `zip` would truncate to the shorter side and a table
    // missing its last reason would pass by describing nothing.
    assert_eq!(
        rows.len(),
        DisplayFault::ALL.len(),
        "every registered reason needs a row before its cells can be checked"
    );
    for (fault, row) in DisplayFault::ALL.into_iter().zip(rows) {
        let expected = expected_cells(fault);
        assert_eq!(
            row[..4],
            expected,
            "documented row for {fault:?} disagrees with the code"
        );
        assert!(
            !row[4].is_empty(),
            "the documented row for {fault:?} must state a meaning"
        );
    }
}

/// The registry's shape, derived from the enum rather than from a
/// parallel list: every reason holds its own slot, `ALL` holds it at
/// that slot, and its code round-trips.
///
/// This is what makes a reused code impossible. A code is a slot, and a
/// duplicate code is a duplicate slot, which fails here — for every
/// reason `ALL` holds. Nothing in Rust can enumerate an enum's
/// variants, so a variant left out of `ALL` would escape this test
/// entirely, and one given an already-used slot would collide with an
/// existing reason's identity, label, and class. `check-structure.sh`
/// closes that by counting: these entries are pairwise distinct, so N
/// of them drawn from N variants means `ALL` is the variant set.
#[test]
fn every_reason_holds_its_own_slot_and_code() {
    let mut seen = [false; DisplayFault::ALL.len()];
    for reason in DisplayFault::ALL {
        let slot = DisplayFault::ALL
            .iter()
            .position(|candidate| *candidate == reason)
            .expect("a reason of ALL is in ALL");
        assert!(!seen[slot], "two reasons share slot {slot}");
        seen[slot] = true;
        assert_eq!(
            reason.code(),
            u8::try_from(slot + 1).expect("registry fits a u8"),
            "a code is its slot, counted from one"
        );
        assert_eq!(
            DisplayFault::from_code(reason.code()),
            Some(reason),
            "every code decodes back to its own reason"
        );
    }
    assert!(seen.iter().all(|held| *held), "ALL has a gap");
}
