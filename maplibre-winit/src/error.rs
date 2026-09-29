//! Host failures retain their operating-system or map initialization causes.
use thiserror::Error;

/// The windowing system could not create a host resource.
#[derive(Debug, Error)]
pub enum WinitHostError {
    /// The operating system rejected the event loop.
    #[error("winit event loop failed")]
    EventLoop(#[source] winit::error::EventLoopError),
    /// The operating system rejected a window on the active event loop.
    #[error("creating resumed winit window failed")]
    Window(#[source] winit::error::OsError),
    /// The browser has no document to attach a canvas to.
    #[cfg(target_arch = "wasm32")]
    #[error("browser document is unavailable")]
    DocumentUnavailable,
    /// No element has the requested canvas ID.
    #[cfg(target_arch = "wasm32")]
    #[error("canvas element {id:?} does not exist")]
    CanvasMissing {
        /// Requested DOM element ID.
        id: String,
    },
    /// The configured element is not a canvas.
    #[cfg(target_arch = "wasm32")]
    #[error("element {id:?} is not a canvas")]
    InvalidCanvas {
        /// Requested DOM element ID.
        id: String,
    },
}

/// A map factory or its window lifecycle failed.
#[derive(Debug, Error)]
pub enum WinitApplicationError<E: std::error::Error + 'static> {
    /// Creating the host event loop or window failed.
    #[error(transparent)]
    Host(#[from] WinitHostError),
    /// The application could not construct its map and services.
    #[error("constructing windowed map failed")]
    CreateMap(#[source] E),
    /// The map could not initialize its renderer while resumed.
    #[error("initializing resumed map failed")]
    Initialize(#[source] maplibre::map::MapError),
    /// A rendered frame failed.
    #[error("rendering windowed map failed")]
    Frame(#[source] maplibre::map::MapError),
    /// The event loop exited before the first successful initialization.
    #[error("event loop closed before map initialization completed")]
    ClosedBeforeReady,
}
