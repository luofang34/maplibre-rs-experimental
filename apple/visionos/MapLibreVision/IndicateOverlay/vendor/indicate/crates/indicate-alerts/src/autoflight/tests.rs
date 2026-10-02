#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::{
    AlertContext, AlertEvent, AlertManager, AlertProfile, AlertState, FlightPhase, InhibitRule,
};

#[test]
fn disconnect_has_a_distinct_aural_and_survives_declutter() {
    let condition = AlertCondition::Autoflight(AutoflightFault::AutopilotDisconnect);
    let profile = AlertProfile::new(
        1000,
        1000,
        1000,
        &[InhibitRule {
            id: condition.id(),
            phase: FlightPhase::Cruise,
        }],
    );
    // Warning inhibition must remain prohibited for the new alert identity.
    assert!(profile.is_err());
    let profile = AlertProfile::new(1000, 1000, 1000, &[]).expect("profile");
    let mut manager = AlertManager::new();
    let context = AlertContext {
        declutter: true,
        ..AlertContext::default()
    };
    let out = manager.step(&profile, &[AlertEvent::Assert(condition)], context, 0);
    assert_eq!(out.aural(), AuralToken::AutopilotDisconnect);
    assert!(!out.active()[0].decluttered);
    assert_eq!(
        manager.step(&profile, &[], context, 1).aural(),
        AuralToken::AutopilotDisconnect
    );
    let out = manager.step(
        &profile,
        &[AlertEvent::Acknowledge(condition.id())],
        context,
        2,
    );
    assert_eq!(out.aural(), AuralToken::Silent);
    assert_eq!(out.active()[0].state, AlertState::Acknowledged);
}

#[test]
fn known_automation_conditions_have_fixed_severity_and_sound() {
    for code in 1..=5 {
        let fault = AutoflightFault::from_code(code).expect("known code");
        let condition = AlertCondition::Autoflight(fault);
        assert_eq!(class_of(condition.id()), Some(condition.class()));
        assert!(aural_of(condition.id()).is_some());
    }
    assert_eq!(class_of(AlertId(0x0806)), None);
    assert_eq!(aural_of(AlertId(0x0806)), None);
}
