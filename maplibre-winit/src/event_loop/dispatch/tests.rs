use std::time::Duration;

use super::{NextAction, NextFrame};

#[test]
fn an_idle_map_asks_for_no_frame() {
    assert_eq!(NextFrame::default().action(), NextAction::Sleep);
}

#[test]
fn each_immediate_reason_asks_for_a_frame() {
    let reasons = [
        NextFrame {
            map: true,
            ..NextFrame::default()
        },
        NextFrame {
            arrivals: true,
            ..NextFrame::default()
        },
        NextFrame {
            input: true,
            ..NextFrame::default()
        },
        NextFrame {
            counting_frames: true,
            ..NextFrame::default()
        },
        NextFrame {
            wait: Some(Duration::ZERO),
            ..NextFrame::default()
        },
    ];
    for reason in reasons {
        assert_eq!(reason.action(), NextAction::Redraw, "{reason:?}");
    }
}

#[test]
fn pending_timer_sleeps_until_due_unless_immediate_work_exists() {
    let delay = Duration::from_millis(40);
    assert_eq!(
        NextFrame {
            wait: Some(delay),
            ..NextFrame::default()
        }
        .action(),
        NextAction::Wait(delay)
    );
    assert_eq!(
        NextFrame {
            map: true,
            wait: Some(delay),
            ..NextFrame::default()
        }
        .action(),
        NextAction::Redraw
    );
}
