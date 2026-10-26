//! Animated camera moves: a host starts one, each frame advances it by the frame time the host
//! writes, and the host learns whether it arrived or stopped short.

use std::time::Duration;

use crate::{
    context::MapContext,
    render::{
        frame_signals,
        view_state::{CameraOptions, FlightCurve, FlightPath},
    },
    tcs::system::SystemResult,
};

/// Names one camera move; a later move has a different name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlightId(u64);

/// How a camera move stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightOutcome {
    /// The camera is on its way.
    Flying,
    /// The camera reached the destination.
    Arrived,
    /// Something else moved the camera, the host stopped the move, or the camera could not be
    /// placed on the way, so the move ended where the camera was.
    Interrupted,
    /// A later move has finished since, so this one's outcome is no longer kept.
    Superseded,
}

/// The move under way.
#[derive(Clone, Copy, Debug)]
struct Flight {
    id: FlightId,
    path: FlightPath,
    /// The height the destination's center rests at when the move names one.
    elevation: Option<f64>,
    /// The center's height where the move started.
    start_elevation: f64,
    /// The frame time of the first frame that advanced the move.
    started: Option<Duration>,
    /// The camera the move placed last; while it flies, only the move changes it.
    placed: crate::render::view_state::ResolvedJump,
}

/// The move under way, and how the last one ended.
#[derive(Default)]
pub(crate) struct CameraFlights {
    latest: u64,
    active: Option<Flight>,
    finished: Option<(FlightId, FlightOutcome)>,
}

impl MapContext {
    /// Moves the camera to `options` along `curve`, as GL JS `easeTo` and `flyTo` do, from the
    /// next frame on, in either navigation mode; a move under way is interrupted. The center's
    /// height eases from where it starts to the height the destination rests at: the elevation
    /// `options` gives, else, where terrain holds the center, the ground loaded there, sea
    /// level until it loads, as [`Self::jump_to`] stands a center. Moving the camera any other
    /// way interrupts the move.
    pub fn fly_to(&mut self, options: CameraOptions, curve: FlightCurve) -> FlightId {
        let flight = Flight {
            id: FlightId(0),
            path: self.view_state.flight_path(options, curve),
            elevation: options.elevation.filter(|meters| meters.is_finite()),
            start_elevation: self.view_state.center_elevation(),
            started: None,
            placed: self.view_state.resolve_jump(CameraOptions::default()),
        };
        let flights = self.world.resources.get_or_init_mut::<CameraFlights>();
        flights.latest = flights.latest.wrapping_add(1);
        let id = FlightId(flights.latest);
        if let Some(previous) = flights.active.replace(Flight { id, ..flight }) {
            flights.finished = Some((previous.id, FlightOutcome::Interrupted));
        }
        frame_signals::mark_dirty(&mut self.world);
        id
    }

    /// How the move `id` stands.
    pub fn flight_outcome(&self, id: FlightId) -> FlightOutcome {
        let Some(flights) = self.world.resources.get::<CameraFlights>() else {
            return FlightOutcome::Superseded;
        };
        match (flights.active, flights.finished) {
            (Some(flight), _) if flight.id == id => FlightOutcome::Flying,
            (_, Some((finished, outcome))) if finished == id => outcome,
            _ => FlightOutcome::Superseded,
        }
    }

    /// Stops the move under way where the camera is, as an interruption.
    pub fn stop_flight(&mut self) {
        finish(self, FlightOutcome::Interrupted);
    }
}

/// Advances the move under way to the frame's time, holding the center's height to the move's
/// own until it ends.
pub(crate) fn camera_flight_system(context: &mut MapContext) -> SystemResult {
    let Some(mut flight) = context
        .world
        .resources
        .get::<CameraFlights>()
        .and_then(|flights| flights.active)
    else {
        return Ok(());
    };
    let current = context.view_state.resolve_jump(CameraOptions::default());
    if !flight.placed.same_as(&current) {
        finish(context, FlightOutcome::Interrupted);
        return Ok(());
    }
    let now = context.frame_time();
    let elapsed = now.saturating_sub(*flight.started.get_or_insert(now));
    let elevation = destination_elevation(context, &flight);
    let options = flight.path.at(elapsed, flight.start_elevation, elevation);
    context.view_state.freeze_center_elevation();
    if let Err(error) = context.view_state.jump_to(options) {
        tracing::warn!(%error, "a camera move could not place the camera");
        finish(context, FlightOutcome::Interrupted);
        return Ok(());
    }
    if flight.path.arrived(elapsed) {
        finish(context, FlightOutcome::Arrived);
        return Ok(());
    }
    flight.placed = context.view_state.resolve_jump(CameraOptions::default());
    if let Some(flights) = context.world.resources.get_mut::<CameraFlights>() {
        flights.active = Some(flight);
    }
    frame_signals::keep_animating(&mut context.world);
    Ok(())
}

/// The height the move's destination rests at, read again each frame as its ground loads.
fn destination_elevation(context: &MapContext, flight: &Flight) -> f64 {
    if let Some(meters) = flight.elevation {
        return meters;
    }
    if !context.view_state.center_held_by_terrain() {
        return context.view_state.center_altitude();
    }
    let (center, zoom) = flight.path.destination();
    crate::terrain::elevation::ground_under(&context.world, center, zoom).unwrap_or(0.0)
}

/// Ends the move under way with `outcome`, handing the center's height back to the terrain.
fn finish(context: &mut MapContext, outcome: FlightOutcome) {
    let Some(flights) = context.world.resources.get_mut::<CameraFlights>() else {
        return;
    };
    if let Some(flight) = flights.active.take() {
        flights.finished = Some((flight.id, outcome));
        context.view_state.thaw_center_elevation();
        frame_signals::mark_dirty(&mut context.world);
    }
}

#[cfg(all(test, feature = "headless"))]
mod tests;
