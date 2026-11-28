use super::*;

#[test]
fn continuous_camera_motion_throttles_to_250ms_and_settles_to_zero_hz() {
    let mut ctrl = PlacementController::default();
    let cfg = PlacementConfig::default();

    // Initial frame with symbols places immediately.
    assert!(ctrl.should_place(
        Duration::from_millis(0),
        14.1,
        cfg,
        PlacementTriggers {
            moved: true,
            new_content: true,
            ..Default::default()
        }
    ));
    ctrl.record_commit(14.1, true, false);

    // 60 Hz pan frames within 250 ms are skipped.
    for ms in [16_u64, 32, 64, 128, 240] {
        assert!(
            !ctrl.should_place(
                Duration::from_millis(ms),
                14.2,
                cfg,
                PlacementTriggers {
                    moved: true,
                    ..Default::default()
                }
            ),
            "expected skip at {ms} ms"
        );
        assert!(ctrl.is_pending());
    }

    // At 256 ms (>= 250 ms), the next placement epoch fires.
    assert!(ctrl.should_place(
        Duration::from_millis(256),
        14.2,
        cfg,
        PlacementTriggers {
            moved: true,
            ..Default::default()
        }
    ));
    ctrl.record_commit(14.2, true, false);
    assert!(!ctrl.is_pending());

    // Once camera motion stops, settled frames stay at 0 Hz indefinitely.
    for ms in [272_u64, 400, 600, 1_000, 5_000] {
        assert!(!ctrl.should_place(
            Duration::from_millis(ms),
            14.2,
            cfg,
            PlacementTriggers::default()
        ));
        assert!(!ctrl.is_pending());
    }
}

#[test]
fn new_content_and_integer_zoom_crossings_use_30ms_fast_debounce() {
    let mut ctrl = PlacementController::default();
    let cfg = PlacementConfig::default();

    assert!(ctrl.should_place(
        Duration::ZERO,
        13.9,
        cfg,
        PlacementTriggers {
            new_content: true,
            ..Default::default()
        }
    ));
    ctrl.record_commit(13.9, true, false);

    // New tile arrives 16 ms later: debounced until 30 ms.
    assert!(!ctrl.should_place(
        Duration::from_millis(16),
        13.9,
        cfg,
        PlacementTriggers {
            new_content: true,
            ..Default::default()
        }
    ));
    assert!(ctrl.should_place(
        Duration::from_millis(32),
        13.9,
        cfg,
        PlacementTriggers::default()
    ));
    ctrl.record_commit(13.9, true, false);

    // Crossing integer zoom 13.9 -> 14.0 also triggers after 30 ms rather than 250 ms.
    assert!(!ctrl.should_place(
        Duration::from_millis(48),
        14.05,
        cfg,
        PlacementTriggers {
            moved: true,
            ..Default::default()
        }
    ));
    assert!(ctrl.should_place(
        Duration::from_millis(64),
        14.05,
        cfg,
        PlacementTriggers {
            moved: true,
            ..Default::default()
        }
    ));
}

#[test]
fn empty_warmup_frames_do_not_delay_first_symbol_tile() {
    let mut ctrl = PlacementController::default();
    let cfg = PlacementConfig::default();

    assert!(ctrl.should_place(Duration::ZERO, 14.0, cfg, PlacementTriggers::default()));
    ctrl.record_commit(14.0, false, false);

    // First real symbol tile at 16 ms places immediately without waiting 30/250 ms.
    assert!(ctrl.should_place(
        Duration::from_millis(16),
        14.0,
        cfg,
        PlacementTriggers {
            new_content: true,
            ..Default::default()
        }
    ));
}

#[test]
fn immediate_config_places_every_changed_frame_without_debounce() {
    let mut ctrl = PlacementController::default();
    let cfg = PlacementConfig::immediate();

    assert!(ctrl.should_place(Duration::ZERO, 14.0, cfg, PlacementTriggers::default()));
    ctrl.record_commit(14.0, true, false);

    assert!(ctrl.should_place(
        Duration::from_millis(5),
        14.0,
        cfg,
        PlacementTriggers {
            moved: true,
            ..Default::default()
        }
    ));
}

#[test]
fn streaming_tiles_during_continuous_motion_keep_250ms_cadence_then_settle_fast() {
    let mut ctrl = PlacementController::default();
    let cfg = PlacementConfig::default();

    assert!(ctrl.should_place(
        Duration::ZERO,
        14.2,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: true,
            new_content: true,
            ..Default::default()
        }
    ));
    // Even with `followup_needed: true` and continuous `new_content: true`, active motion must
    // stay at 250 ms (`4 Hz`) rather than collapsing to 30 ms (`33 Hz`).
    ctrl.record_commit(14.2, true, true);

    for ms in [16_u64, 32, 48, 80, 128, 200, 240] {
        assert!(
            !ctrl.should_place(
                Duration::from_millis(ms),
                14.3,
                cfg,
                PlacementTriggers {
                    moved: true,
                    moving_this_frame: true,
                    new_content: true,
                    ..Default::default()
                }
            ),
            "streaming tile arrival during continuous flight must not trigger early placement at {ms} ms"
        );
    }

    // At 256 ms (>= 250 ms), the next 4 Hz motion epoch commits.
    assert!(ctrl.should_place(
        Duration::from_millis(256),
        14.3,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: true,
            new_content: true,
            ..Default::default()
        }
    ));
    ctrl.record_commit(14.3, true, false);

    // One more tile uploads while moving at 272 ms, then camera stops at 290 ms (> 30 ms after 256 ms).
    assert!(!ctrl.should_place(
        Duration::from_millis(272),
        14.35,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: true,
            new_content: true,
            ..Default::default()
        }
    ));
    assert!(ctrl.should_place(
        Duration::from_millis(290),
        14.35,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: false,
            ..Default::default()
        }
    ));
}

#[test]
fn integer_zoom_crossings_during_continuous_zoom_flight_keep_250ms_then_settle_at_30ms() {
    let mut ctrl = PlacementController::default();
    let cfg = PlacementConfig::default();

    assert!(ctrl.should_place(
        Duration::ZERO,
        4.8,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: true,
            ..Default::default()
        }
    ));
    ctrl.record_commit(4.8, true, false);

    // Crossing z4 -> z5 -> z6 while `moving_this_frame` is true stays on 250 ms cadence.
    for (ms, zoom) in [(32_u64, 5.1), (80, 5.6), (144, 6.05), (224, 6.4)] {
        assert!(!ctrl.should_place(
            Duration::from_millis(ms),
            zoom,
            cfg,
            PlacementTriggers {
                moved: true,
                moving_this_frame: true,
                ..Default::default()
            }
        ));
    }
    // Motion pauses at 240 ms (>= 30 ms after 0 ms): recorded zoom-crossing followup settles immediately.
    assert!(ctrl.should_place(
        Duration::from_millis(240),
        6.4,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: false,
            ..Default::default()
        }
    ));
}

#[test]
fn transient_empty_frames_preserve_ever_placed_while_aborted_motion_keeps_cadence() {
    let mut ctrl = PlacementController::default();
    let cfg = PlacementConfig::default();

    assert!(!ctrl.has_ever_placed());
    assert!(ctrl.should_place(Duration::ZERO, 14.0, cfg, PlacementTriggers::default()));
    ctrl.record_commit(14.0, true, false);
    assert!(ctrl.has_ever_placed());
    assert!(ctrl.is_initialized());

    // A transient empty-symbol frame clears `initialized` for fast recovery but preserves `has_ever_placed`.
    ctrl.record_commit(14.0, false, false);
    assert!(!ctrl.is_initialized());
    assert!(ctrl.has_ever_placed());

    // Simulate recommit + rapid-zoom abort (`note_aborted_motion`): holds 250 ms cadence while moving.
    assert!(ctrl.should_place(
        Duration::from_millis(16),
        14.2,
        cfg,
        PlacementTriggers::default()
    ));
    ctrl.record_commit(14.2, true, false);
    ctrl.advance_clock(Duration::from_millis(48));
    ctrl.note_aborted_motion(14.9);

    assert!(!ctrl.should_place(
        Duration::from_millis(100),
        15.1,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: true,
            new_content: true,
            ..Default::default()
        }
    ));
    // Pausing >= 30 ms after the abort triggers the 30 ms settle placement pass.
    assert!(ctrl.should_place(
        Duration::from_millis(100),
        15.1,
        cfg,
        PlacementTriggers {
            moved: true,
            moving_this_frame: false,
            ..Default::default()
        }
    ));
}
