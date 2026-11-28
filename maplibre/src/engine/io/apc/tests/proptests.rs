use std::{sync::mpsc, time::Duration};

use proptest::prelude::*;

use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{AttemptContext, Context, Message, SchedulerContext},
        source_client::SourceFetchError,
        source_type::InvalidTileCoords,
        tile_retry::{
            accepts, completed, due, is_pending, needs_frame, next_attempt, next_retry_delay,
            started, RequestDisposition, RequestKind, TileRequestOutcome,
        },
    },
    render::frame_input::FrameInput,
    tcs::world::World,
};

fn request_kind(idx: usize) -> RequestKind {
    match idx {
        0 => RequestKind::Raster,
        1 => RequestKind::Dem,
        _ => RequestKind::Vector,
    }
}

fn tile_coords(x: i32, y: i32, z: u8) -> WorldTileCoords {
    let span = 1i32 << z;
    WorldTileCoords::from((x.rem_euclid(span), y.rem_euclid(span), z.into()))
}

fn set_time_ms(world: &mut World, ms: u64) {
    world.resources.get_or_init_mut::<FrameInput>().timestamp = Duration::from_millis(ms);
}

proptest! {
    #[test]
    fn prop_tile_retry_backoff_bounded_and_monotonic(
        x in 0i32..64,
        y in 0i32..64,
        z in 0u8..5,
        kind_idx in 0usize..3,
        steps in 1usize..=12,
        base_ms in 0u64..100_000,
    ) {
        let mut world = World::default();
        let coords = tile_coords(x, y, z);
        let kind = request_kind(kind_idx);
        let mut now_ms = base_ms;
        let mut prev_delay = Duration::ZERO;

        for _ in 0..steps {
            set_time_ms(&mut world, now_ms);
            let attempt = next_attempt(&mut world);
            started(&mut world, coords, kind, attempt, None);
            completed(
                &mut world,
                TileRequestOutcome {
                    coords,
                    kind,
                    attempt: Some(attempt),
                    disposition: RequestDisposition::Retry,
                },
            );
            let Some(delay) = next_retry_delay(&world) else {
                return Err(TestCaseError::fail("retry must schedule a delay"));
            };
            prop_assert!(delay >= Duration::from_secs(1));
            prop_assert!(delay <= Duration::from_secs(30));
            prop_assert!(delay >= prev_delay);
            prop_assert!(!due(&mut world, coords, kind));

            let wait_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
            now_ms = now_ms.saturating_add(wait_ms);
            set_time_ms(&mut world, now_ms);
            prop_assert!(due(&mut world, coords, kind));
            prev_delay = delay;
        }
    }
}

proptest! {
    #[test]
    fn prop_tile_retry_permanent_never_schedules_retry(
        x in 0i32..64,
        y in 0i32..64,
        z in 0u8..5,
        kind_idx in 0usize..3,
        status in 100u16..600,
        prior_retries in 0usize..=4,
        advance_secs in 0u64..100_000,
    ) {
        let http_err = SourceFetchError::http_response(
            "https://example.invalid/tile",
            status,
            std::io::Error::other("status"),
        );
        let expect_retry = status == 408 || status == 429 || (500..600).contains(&status);
        prop_assert_eq!(http_err.is_retryable(), expect_retry);
        prop_assert!(!SourceFetchError::not_found("https://example.invalid/tile").is_retryable());
        let coords = tile_coords(x, y, z);
        let kind = request_kind(kind_idx);
        let invalid_err = SourceFetchError(Box::new(InvalidTileCoords { coords }));
        prop_assert!(!invalid_err.is_retryable());
        let mut world = World::default();
        let mut now_ms = 1_000u64;

        for _ in 0..prior_retries {
            set_time_ms(&mut world, now_ms);
            let attempt = next_attempt(&mut world);
            started(&mut world, coords, kind, attempt, None);
            completed(
                &mut world,
                TileRequestOutcome {
                    coords,
                    kind,
                    attempt: Some(attempt),
                    disposition: RequestDisposition::Retry,
                },
            );
            now_ms = now_ms.saturating_add(30_000);
        }

        set_time_ms(&mut world, now_ms);
        let final_attempt = next_attempt(&mut world);
        started(&mut world, coords, kind, final_attempt, None);
        completed(
            &mut world,
            TileRequestOutcome {
                coords,
                kind,
                attempt: Some(final_attempt),
                disposition: RequestDisposition::Complete,
            },
        );

        prop_assert!(!is_pending(&world, coords, kind));
        prop_assert!(!due(&mut world, coords, kind));
        prop_assert_eq!(next_retry_delay(&world), None);
        prop_assert!(!needs_frame(&world));

        set_time_ms(&mut world, now_ms.saturating_add(advance_secs.saturating_mul(1_000)));
        prop_assert!(!due(&mut world, coords, kind));
        prop_assert_eq!(next_retry_delay(&world), None);
        prop_assert!(!needs_frame(&world));
    }
}

proptest! {
    #[test]
    fn prop_apc_epoch_supersede_drops_stale_responses(
        x in 0i32..64,
        y in 0i32..64,
        z in 0u8..5,
        kind_idx in 0usize..3,
        epochs in 2usize..=8,
        disposition_retry in any::<bool>(),
    ) {
        let mut world = World::default();
        let coords = tile_coords(x, y, z);
        let kind = request_kind(kind_idx);
        let (tx, rx) = mpsc::channel();
        let base_ctx = SchedulerContext::new(tx, None);

        let mut issued = Vec::with_capacity(epochs);
        for _ in 0..epochs {
            let attempt = next_attempt(&mut world);
            started(&mut world, coords, kind, attempt, None);
            let attempt_ctx = AttemptContext::new(base_ctx.clone(), attempt);
            let msg = Message::new(kind.message_tag(), Box::new(attempt));
            prop_assert!(attempt_ctx.send_back(msg).is_ok());
            issued.push(attempt);
        }

        let Some(&latest) = issued.last() else {
            return Err(TestCaseError::fail("expected non-empty attempt list"));
        };
        let disposition = if disposition_retry {
            RequestDisposition::Retry
        } else {
            RequestDisposition::Complete
        };

        for &attempt in &issued {
            let Ok(delivered) = rx.try_recv() else {
                return Err(TestCaseError::fail("expected stamped message"));
            };
            prop_assert_eq!(delivered.attempt(), Some(attempt));
            if attempt != latest {
                prop_assert!(!accepts(&world, coords, kind, Some(attempt)));
                completed(
                    &mut world,
                    TileRequestOutcome {
                        coords,
                        kind,
                        attempt: Some(attempt),
                        disposition,
                    },
                );
                prop_assert!(is_pending(&world, coords, kind));
                prop_assert!(accepts(&world, coords, kind, Some(latest)));
                prop_assert_eq!(next_retry_delay(&world), None);
            }
        }

        prop_assert!(!accepts(&world, coords, kind, None));
        prop_assert!(accepts(&world, coords, kind, Some(latest)));
        completed(
            &mut world,
            TileRequestOutcome {
                coords,
                kind,
                attempt: Some(latest),
                disposition: RequestDisposition::Complete,
            },
        );
        prop_assert!(!is_pending(&world, coords, kind));
    }
}

proptest! {
    #[test]
    fn prop_tile_retry_ready_set_idempotent_under_time_advance(
        tile_specs in prop::collection::vec((0i32..16, 0i32..16, 0usize..3, any::<bool>()), 1..=6),
        start_ms in 1_000u64..10_000,
        extra_ms in 0u64..5_000,
        repeats in 2usize..=6,
    ) {
        let mut world = World::default();
        set_time_ms(&mut world, start_ms);

        let mut seen = Vec::new();
        for (x, y, kind_idx, should_retry) in tile_specs {
            let coords = tile_coords(x, y, 4);
            let kind = request_kind(kind_idx);
            if seen.iter().any(|&(c, k)| c == coords && k == kind) {
                continue;
            }
            seen.push((coords, kind));

            let attempt = next_attempt(&mut world);
            started(&mut world, coords, kind, attempt, None);
            let disposition = if should_retry {
                RequestDisposition::Retry
            } else {
                RequestDisposition::Complete
            };
            completed(
                &mut world,
                TileRequestOutcome {
                    coords,
                    kind,
                    attempt: Some(attempt),
                    disposition,
                },
            );
        }

        let probe_ms = start_ms.saturating_add(1_000).saturating_add(extra_ms);
        set_time_ms(&mut world, probe_ms);

        let initial_ready: Vec<bool> = seen
            .iter()
            .map(|&(coords, kind)| due(&mut world, coords, kind))
            .collect();
        let initial_delay = next_retry_delay(&world);
        let initial_needs_frame = needs_frame(&world);

        for step in 0..repeats {
            set_time_ms(&mut world, probe_ms.saturating_add(step as u64));
            let next_ready: Vec<bool> = seen
                .iter()
                .map(|&(coords, kind)| due(&mut world, coords, kind))
                .collect();
            prop_assert_eq!(&next_ready, &initial_ready);
            prop_assert_eq!(next_retry_delay(&world), initial_delay);
            prop_assert_eq!(needs_frame(&world), initial_needs_frame);
        }

        // Setting FrameInput backwards never rolls back deadlines or un-dues ready tiles.
        set_time_ms(&mut world, 0);
        let after_rewind: Vec<bool> = seen
            .iter()
            .map(|&(coords, kind)| due(&mut world, coords, kind))
            .collect();
        prop_assert_eq!(&after_rewind, &initial_ready);
        prop_assert_eq!(next_retry_delay(&world), initial_delay);
    }
}
