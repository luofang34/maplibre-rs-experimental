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
            retry: Some(Duration::ZERO),
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
fn pending_input_and_retry_timers_sleep_until_the_earliest_deadline() {
    let input_delay = Duration::from_millis(40);
    let retry_delay = Duration::from_secs(1);
    assert_eq!(
        NextFrame {
            retry: Some(retry_delay),
            ..NextFrame::default()
        }
        .action(),
        NextAction::Wait(retry_delay)
    );
    assert_eq!(
        NextFrame {
            wait: Some(input_delay),
            retry: Some(retry_delay),
            ..NextFrame::default()
        }
        .action(),
        NextAction::Wait(input_delay)
    );
    assert_eq!(
        NextFrame {
            map: true,
            retry: Some(retry_delay),
            ..NextFrame::default()
        }
        .action(),
        NextAction::Redraw
    );
}
