#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use indicate_instrument_descriptor::PanelDrawError;
use indicate_instrument_scene::{Cmd, SceneCmds, SceneWriter};
use indicate_instrument_state::{FreshnessPolicy, abi::v8::fixtures, resolve};
use std::vec::Vec;

fn data() -> PanelData {
    resolve(&fixtures::extended(), &FreshnessPolicy::default())
}

fn supplemental() -> DisplayContext {
    DisplayContext {
        role: DisplayRole::Supplemental,
        region: ViewRegion::Hud,
        alternate_flight_display: Sig::valid(true),
        ..DisplayContext::default()
    }
}

fn render(data: &PanelData, context: DisplayContext) -> Result<Vec<u8>, PanelDrawError> {
    let mut storage = std::vec![0; 8192];
    let mut writer = SceneWriter::new(&mut storage)?;
    crate::draw_hwd(data, context, false, None, &mut writer)?;
    let length = writer.finish();
    storage.truncate(length);
    Ok(storage)
}

fn texts(bytes: &[u8]) -> Vec<&str> {
    SceneCmds::new(bytes)
        .expect("scene")
        .filter_map(|c| match c.expect("command") {
            Cmd::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn blanking_requires_supplemental_role_and_current_visibility_evidence() {
    let data = data();
    let context = supplemental();
    assert!(presentation_plan(&data, context, None).blank_flight);
    for invalid in [
        DisplayContext {
            role: DisplayRole::Primary,
            ..context
        },
        DisplayContext {
            region: ViewRegion::Outside,
            ..context
        },
        DisplayContext {
            alternate_flight_display: Sig::valid(false),
            ..context
        },
    ] {
        assert!(!presentation_plan(&data, invalid, None).blank_flight);
    }
    for status in [
        SignalStatus::Missing,
        SignalStatus::Stale,
        SignalStatus::Degraded,
        SignalStatus::Failed,
    ] {
        let invalid = DisplayContext {
            alternate_flight_display: Sig::with_status(true, status),
            ..context
        };
        assert!(!presentation_plan(&data, invalid, None).blank_flight);
    }
}

#[test]
fn recovery_and_failed_flight_references_override_blanking() {
    let mut data = data();
    let context = DisplayContext {
        task: DisplayTask::Mission,
        ..supplemental()
    };
    data.presentation.unusual = true;
    let plan = presentation_plan(&data, context, None);
    assert!(plan.recovery);
    assert!(!plan.blank_flight && !plan.mission && !plan.detailed);
    assert!(texts(&render(&data, context).expect("recovery")).contains(&"HWD SUPP RECOVERY"));
    data.presentation.unusual = false;
    data.roll_rad.status = SignalStatus::Failed;
    assert!(!presentation_plan(&data, context, None).blank_flight);
    let scene = render(&data, context).expect("failed attitude");
    assert!(texts(&scene).contains(&"ATT FAILED"));
}

#[test]
fn blanked_view_retains_modes_and_restore_on_visibility_loss() {
    let data = data();
    let blank = render(&data, supplemental()).expect("blanked");
    let labels = texts(&blank);
    for required in [
        "AP ON",
        "FD ON",
        "A/THR ON",
        "NAV",
        "APR",
        "HWD SUPP HUD BLANK",
    ] {
        assert!(labels.contains(&required), "{required}");
    }
    assert!(!labels.contains(&"282"));
    let restored = render(
        &data,
        DisplayContext {
            alternate_flight_display: Sig::with_status(false, SignalStatus::Missing),
            ..supplemental()
        },
    )
    .expect("restored");
    assert!(texts(&restored).contains(&"282"));
}

#[test]
fn mission_reservation_replaces_receiver_detail_but_retains_flight_control() {
    let data = data();
    let context = DisplayContext {
        task: DisplayTask::Mission,
        ..DisplayContext::default()
    };
    let scene = render(&data, context).expect("mission");
    let labels = texts(&scene);
    for required in [
        "HWD PRIMARY MISSION",
        "282",
        "TGT M 0.780",
        "AP ON",
        "FD ATTITUDE",
    ] {
        assert!(labels.contains(&required), "{required}: {labels:?}");
    }
    assert!(!labels.contains(&"CDI"));
    assert!(!labels.contains(&"GS KT"));
    let zone = crate::MISSION_ZONE;
    for command in SceneCmds::new(&scene).expect("scene") {
        if let Cmd::Text { x, y, .. } = command.expect("command") {
            assert!(
                !(x >= zone.x
                    && x <= zone.x + zone.width
                    && y >= zone.y
                    && y <= zone.y + zone.height)
            );
        }
    }
}

#[test]
fn transition_uses_producer_age_and_never_draws_a_wide_mode_box() {
    let mut data = data();
    let fresh = render(&data, DisplayContext::default()).expect("transition");
    assert!(texts(&fresh).contains(&"MODE CAPTURE"));
    let mut wide = false;
    for command in SceneCmds::new(&fresh).expect("scene") {
        if let Cmd::Rect { y, w, .. } = command.expect("command") {
            wide |= y < 120.0 && w > 160.0;
        }
    }
    assert!(!wide);
    data.flight.guidance_age_ms = Some(5000.0);
    let aged = render(&data, DisplayContext::default()).expect("aged");
    assert!(!texts(&aged).contains(&"MODE CAPTURE"));
    assert!(texts(&aged).contains(&"AP ON"));
}

#[test]
fn status_is_head_fixed_in_every_layout() {
    for compact in [false, true] {
        assert_eq!(
            layer_reference(LayerId::Annunciation, compact),
            InstrumentReference::Head
        );
        assert_eq!(
            layer_reference(LayerId::Failure, compact),
            InstrumentReference::Head
        );
    }
    assert_eq!(
        layer_reference(LayerId::Tapes, false),
        InstrumentReference::Head
    );
    assert_eq!(
        layer_reference(LayerId::Tapes, true),
        InstrumentReference::Head
    );
}

#[test]
fn urgent_alerts_restore_flight_and_suspend_mission_content() {
    use indicate_alerts::*;
    let data = data();
    let context = DisplayContext {
        task: DisplayTask::Mission,
        ..supplemental()
    };
    for fault in [
        AutoflightFault::AutopilotDisconnect,
        AutoflightFault::ModeReversion,
    ] {
        let alerts = AlertManager::new().step(
            &AlertProfile::simulator(),
            &[AlertEvent::Assert(AlertCondition::Autoflight(fault))],
            AlertContext::default(),
            0,
        );
        let plan = presentation_plan(&data, context, Some(&alerts));
        assert!(!plan.blank_flight && !plan.mission);
        let mut storage = [0; 8192];
        let mut writer = SceneWriter::new(&mut storage).expect("buffer");
        crate::draw_hwd(&data, context, false, Some(&alerts), &mut writer).expect("alert scene");
        let length = writer.finish();
        let labels = texts(&storage[..length]);
        let expected = if fault == AutoflightFault::AutopilotDisconnect {
            "AP DISC"
        } else {
            "MODE REVERSION"
        };
        assert!(labels.contains(&expected));
        assert!(labels.contains(&"282"));
        assert!(labels.contains(&"HWD SUPP FLIGHT PRIORITY"));
    }
}

#[test]
fn automation_text_stays_inside_its_reservation() {
    let data = data();
    for compact in [false, true] {
        let mut storage = [0; 8192];
        let mut writer = SceneWriter::new(&mut storage).expect("buffer");
        crate::draw_hwd(&data, DisplayContext::default(), compact, None, &mut writer)
            .expect("scene");
        let length = writer.finish();
        let zone = crate::AUTOMATION_ZONE;
        for command in SceneCmds::new(&storage[..length]).expect("decode") {
            if let Cmd::Text {
                x, y, size, text, ..
            } = command.expect("command")
                && [
                    "AP ON",
                    "FD ON",
                    "A/THR ON",
                    "NAV",
                    "ALT",
                    "SPD",
                    "APR",
                    "GS",
                    "MODE CAPTURE",
                ]
                .contains(&text)
            {
                let width =
                    text.len() as f32 * f32::from(indicate_instrument_glyphs::ADVANCE) * size
                        / indicate_instrument_glyphs::CELL_H as f32;
                assert!(x - width / 2.0 >= zone.x && x + width / 2.0 <= zone.x + zone.width);
                assert!(y - size / 2.0 >= zone.y && y + size / 2.0 <= zone.y + zone.height);
            }
        }
    }
}

#[test]
fn mission_text_footprints_leave_the_reserved_region_clear_in_both_layouts() {
    use indicate_instrument_scene::{HAlign, VAlign};
    let mut data = data();
    let context = DisplayContext {
        task: DisplayTask::Mission,
        ..DisplayContext::default()
    };
    for compact in [false, true] {
        for pressure in [1013.0, 999999.0] {
            data.baro_hpa = Sig::with_status(pressure, SignalStatus::Stale);
            let mut storage = [0; 8192];
            let mut writer = SceneWriter::new(&mut storage).expect("buffer");
            crate::draw_hwd(&data, context, compact, None, &mut writer).expect("mission");
            let length = writer.finish();
            for command in SceneCmds::new(&storage[..length]).expect("scene") {
                if let Cmd::Text {
                    x,
                    y,
                    size,
                    text,
                    anchor,
                } = command.expect("command")
                {
                    let width =
                        text.len() as f32 * f32::from(indicate_instrument_glyphs::ADVANCE) * size
                            / indicate_instrument_glyphs::CELL_H as f32;
                    let left = x - match anchor.h {
                        HAlign::Left => 0.0,
                        HAlign::Center => width / 2.0,
                        HAlign::Right => width,
                    };
                    let top = y - match anchor.v {
                        VAlign::Top => 0.0,
                        VAlign::Middle => size / 2.0,
                        _ => size,
                    };
                    let zone = crate::MISSION_ZONE;
                    let overlaps = left < zone.x + zone.width
                        && left + width > zone.x
                        && top < zone.y + zone.height
                        && top + size > zone.y;
                    assert!(
                        !overlaps,
                        "mission region contains {text} in compact={compact}"
                    );
                }
            }
        }
    }
}

#[test]
fn alert_manager_failure_and_warning_fit_the_alert_reservation() {
    use indicate_alerts::*;
    let alerts = AlertManager::new().step(
        &AlertProfile::simulator(),
        &[AlertEvent::Assert(AlertCondition::Autoflight(
            AutoflightFault::AutopilotDisconnect,
        ))],
        AlertContext {
            alerting_path_healthy: false,
            ..AlertContext::default()
        },
        0,
    );
    let mut storage = [0; 8192];
    let mut writer = SceneWriter::new(&mut storage).expect("buffer");
    crate::draw_hwd(&data(), supplemental(), false, Some(&alerts), &mut writer).expect("alerts");
    let length = writer.finish();
    let zone = crate::ALERT_ZONE;
    let labels = texts(&storage[..length]);
    assert!(labels.contains(&"ALRT FAIL") && labels.contains(&"AP DISC"));
    for command in SceneCmds::new(&storage[..length]).expect("scene") {
        if let Cmd::Text {
            x, y, size, text, ..
        } = command.expect("command")
            && ["ALRT FAIL", "AP DISC"].contains(&text)
        {
            let width = text.len() as f32 * f32::from(indicate_instrument_glyphs::ADVANCE) * size
                / indicate_instrument_glyphs::CELL_H as f32;
            assert!(x >= zone.x && x + width <= zone.x + zone.width);
            assert!(y - size >= zone.y && y <= zone.y + zone.height);
        }
    }
}
