#![allow(clippy::expect_used, clippy::panic)]
use super::{FRAME, HMD_DESCRIPTOR, HMD_SET};
use indicate_instrument_conformance::admit;
use indicate_instrument_descriptor::EMPTY_CONFIG;
use indicate_instrument_registry::{PanelSet, Registry};
use indicate_instrument_scene::{Cmd, LayerId, SceneCmds, SceneWriter};
use indicate_instrument_state::{AircraftState, FreshnessPolicy, resolve};
use std::vec::Vec;

#[test]
fn independent_overlay_passes_admission_without_background_or_overflow() {
    static SETS: [&PanelSet; 1] = [&HMD_SET];
    let registry = Registry::from_sets(&SETS).expect("independent set composes");
    let report = admit(&registry).expect("transparent readouts pass source withholding");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn missing_air_data_draws_missing_not_a_numeral() {
    let data = resolve(&AircraftState::default(), &FreshnessPolicy::default());
    let mut storage = [0; 8192];
    let mut writer = SceneWriter::new(&mut storage).expect("scene buffer");
    (HMD_DESCRIPTOR.draw)(&data, &EMPTY_CONFIG, None, FRAME, &mut writer)
        .expect("missing-data scene");
    let size = writer.finish();
    let commands: Vec<_> = SceneCmds::new(&storage[..size])
        .expect("decode")
        .map(|cmd| cmd.expect("valid command"))
        .collect();
    assert!(!commands.iter().any(|cmd| matches!(
        cmd,
        Cmd::BeginLayer {
            layer: LayerId::Background
        }
    )));
    let texts: Vec<_> = commands
        .iter()
        .filter_map(|cmd| match cmd {
            Cmd::Text { text, .. } => Some(*text),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&"IAS KT"));
    assert!(texts.contains(&"MISSING"));
    assert!(texts.contains(&"ATT MISSING"));
    assert!(texts.contains(&"---"));
    assert!(
        !texts
            .iter()
            .any(|text| text.chars().any(|c| c.is_ascii_digit()))
    );
}
