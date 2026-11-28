//! Delivery of user events to a running host.

#![deny(missing_docs)]

use thiserror::Error;

/// Delivery was rejected because the event loop cannot receive more events.
#[derive(Error, Debug)]
pub enum SendEventError {
    /// The event loop was already closed
    #[error("event loop is closed")]
    Closed,
}

/// Delivers user events without borrowing the running map.
pub trait EventLoopProxy<T: 'static> {
    /// Enqueues an event or returns [`SendEventError::Closed`] if the loop has shut down.
    fn send_event(&self, event: T) -> Result<(), SendEventError>;
}
