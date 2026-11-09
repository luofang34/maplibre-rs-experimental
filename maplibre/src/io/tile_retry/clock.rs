//! The time retry deadlines are measured in: the host's frame time when it writes one, the
//! wall clock otherwise, never running backwards.

use std::time::Duration;

use instant::Instant;

use crate::{render::frame_input::FrameInput, tcs::world::World};

struct RequestClock {
    started: Instant,
    last: Duration,
}

impl Default for RequestClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            last: Duration::ZERO,
        }
    }
}

pub(super) fn now(world: &mut World) -> Duration {
    world.resources.get_or_init_mut::<RequestClock>();
    let current = observed_now(world);
    world.resources.get_or_init_mut::<RequestClock>().last = current;
    current
}

pub(super) fn observed_now(world: &World) -> Duration {
    let timestamp = world
        .resources
        .get::<FrameInput>()
        .map(|input| input.timestamp);
    world.resources.get::<RequestClock>().map_or_else(
        || timestamp.unwrap_or(Duration::ZERO),
        |clock| {
            clock
                .last
                .max(timestamp.unwrap_or_else(|| clock.started.elapsed()))
        },
    )
}
