//! Host event-loop execution and delivery of user events.

#![deny(missing_docs)]

use thiserror::Error;

use crate::{
    environment::Environment,
    map::Map,
    window::{HeadedMapWindow, MapWindowConfig},
};

/// Associates a host configuration with its user-event type and sender.
pub trait EventLoopConfig {
    /// Payload accepted by the host event loop.
    type EventType: 'static;
    /// Sender that delivers events to the corresponding host loop.
    type EventLoopProxy: EventLoopProxy<Self::EventType>;

    /// Creates a sender using the implementation's host-loop configuration.
    fn create_proxy() -> Self::EventLoopProxy;
}

/// Delivery was rejected because the event loop cannot receive more events.
#[derive(Error, Debug)]
pub enum SendEventError {
    /// The event loop was already closed
    #[error("event loop is closed")]
    Closed,
}

/// The host event loop could not run.
#[derive(Error, Debug)]
#[error("event loop execution failed")]
pub struct EventLoopError;

/// Delivers user events without borrowing the running map.
pub trait EventLoopProxy<T: 'static> {
    /// Enqueues an event or returns [`SendEventError::Closed`] if the loop has shut down.
    fn send_event(&self, event: T) -> Result<(), SendEventError>;
}

/// Transfers a map to the host's event dispatch and redraw loop.
pub trait EventLoop<ET: 'static + PartialEq> {
    /// Sender that delivers events to the corresponding host loop.
    type EventLoopProxy: EventLoopProxy<ET>;

    /// Starts dispatching events, with an optional host-enforced frame limit.
    /// Native hosts block until exit; browser hosts may return after registering callbacks.
    fn run<E>(self, map: Map<E>, max_frames: Option<u64>) -> Result<(), EventLoopError>
    where
        E: Environment,
        <E::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow;

    /// Creates a sender targeting this loop.
    fn create_proxy(&self) -> Self::EventLoopProxy;
}
