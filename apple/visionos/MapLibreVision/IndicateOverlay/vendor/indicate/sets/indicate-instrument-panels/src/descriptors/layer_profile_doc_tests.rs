//! The required-panel-profiles table in the layer-protocol document is
//! prose a backend author trusts, so it is checked against the shipped
//! descriptors rather than maintained beside them.

#![allow(clippy::expect_used, clippy::panic)]

use std::format;
use std::string::String;
use std::vec::Vec;

use indicate_instrument_descriptor::PanelDescriptor;
use indicate_instrument_scene::{LAYER_COUNT, LayerId};

use super::{ALL_SETS, layer_bit};

const PROTOCOL_DOC: &str = include_str!("../../../../docs/instruments/scene-layer-protocol.md");

const HEADING: &str = "## Required panel profiles";

const HEADER: [&str; 3] = ["Panel", "Required layers", "Optional layers"];

/// Every defined layer, ascending — the order both table columns list.
fn defined_layers() -> Vec<LayerId> {
    (0..LAYER_COUNT as u8)
        .map(|id| LayerId::from_u8(id).expect("ids below LAYER_COUNT are defined"))
        .collect()
}

/// The two cells a descriptor's mask implies: the required layers, and
/// its complement over the defined layers.
fn cells_from_mask(panel: &PanelDescriptor) -> (String, String) {
    let (required, optional): (Vec<LayerId>, Vec<LayerId>) = defined_layers()
        .into_iter()
        .partition(|layer| panel.required_layers & layer_bit(*layer) != 0);
    (backticked(&required), backticked(&optional))
}

fn backticked(layers: &[LayerId]) -> String {
    layers
        .iter()
        .map(|layer| format!("`{layer:?}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The profiles section alone, bounded by the next section's heading:
/// a table elsewhere in the document can never be read as a profile row,
/// and deleting the real table fails loudly instead of silently
/// latching onto the next one.
fn section() -> &'static str {
    let after = PROTOCOL_DOC
        .split_once(HEADING)
        .expect("the layer-protocol document still has a required-profiles section")
        .1;
    after
        .split_once("\n## ")
        .map_or(after, |(section, _)| section)
}

fn cells(row: &str) -> Vec<&str> {
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
    assert_eq!(
        cells.len(),
        HEADER.len(),
        "a profiles row must carry panel, required, and optional: {row}"
    );
    cells
}

/// Lines of the section that belong to a markdown table.
fn table_lines() -> Vec<&'static str> {
    section()
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .collect()
}

/// The table's data rows as (panel, required, optional).
///
/// Parsing is positional — header, delimiter, then data — so a row
/// cannot exempt itself from the comparison by imitating the header or
/// leaving its cells blank.
fn documented_rows() -> Vec<(String, String, String)> {
    let lines = table_lines();
    let mut lines = lines.into_iter();
    let header = lines
        .next()
        .expect("the profiles section still contains a table");
    assert_eq!(
        cells(header),
        HEADER.to_vec(),
        "the profiles table's columns must stay panel, required, optional"
    );
    let delimiter = lines.next().expect("the table still has a delimiter row");
    assert!(
        cells(delimiter)
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-')),
        "the row under the header must be the table delimiter: {delimiter}"
    );
    lines
        .map(|line| {
            let cells = cells(line);
            (
                String::from(cells[0]),
                String::from(cells[1]),
                String::from(cells[2]),
            )
        })
        .collect()
}

/// A second table in the section could assert profiles nothing checks.
#[test]
fn the_profiles_section_holds_exactly_one_table() {
    let expected = shipped_panels().len() + 2;
    assert_eq!(
        table_lines().len(),
        expected,
        "the profiles section must hold one table: a header, a delimiter, \
         and one row per shipped panel"
    );
}

/// Every panel this crate ships, in table order, taken from
/// [`super::ALL_SETS`] rather than from a list written here.
///
/// A check that names the sets it walks cannot see a panel in a set it
/// does not name, and the next set added is the one it will not name.
/// Reading the registry instead makes the table's coverage follow what
/// the crate ships.
fn shipped_panels() -> Vec<&'static PanelDescriptor> {
    ALL_SETS.iter().flat_map(|set| set.panels.iter()).collect()
}

#[test]
fn every_shipped_panel_has_a_row_in_shell_order() {
    let documented: Vec<String> = documented_rows().into_iter().map(|row| row.0).collect();
    let shipped: Vec<String> = shipped_panels()
        .into_iter()
        .map(|panel| String::from(panel.title))
        .collect();
    assert_eq!(
        documented, shipped,
        "the profiles table must list every shipped panel, in shell display order"
    );
}

#[test]
fn the_documented_profiles_are_the_descriptor_masks() {
    let rows = documented_rows();
    // Without this, `zip` would truncate to the shorter side and a table
    // missing its last panel would pass by describing nothing.
    assert_eq!(
        rows.len(),
        shipped_panels().len(),
        "every shipped panel needs a row before its mask can be checked"
    );
    for (panel, row) in shipped_panels().into_iter().zip(rows) {
        let (required, optional) = cells_from_mask(panel);
        assert_eq!(
            row.1, required,
            "{}: documented required layers disagree with required_layers",
            panel.title
        );
        assert_eq!(
            row.2, optional,
            "{}: documented optional layers are not the complement of required_layers",
            panel.title
        );
    }
}

/// The comparison guards drift only if a changed mask changes the cell
/// it compares, so drift cannot hide behind an unchanged string.
#[test]
fn a_changed_mask_changes_the_cell_the_guard_compares() {
    let panels = shipped_panels();
    let panel = panels.first().expect("the family ships at least one panel");
    let rows = documented_rows();
    let row = rows.first().expect("that panel has a row");
    let mut drifted = **panel;
    drifted.required_layers &= !layer_bit(LayerId::Annunciation);
    assert_eq!(
        cells_from_mask(panel).0,
        row.1,
        "the shipped mask must match the row the guard reads"
    );
    assert_ne!(
        cells_from_mask(&drifted).0,
        row.1,
        "dropping a required layer must break that match"
    );
}
