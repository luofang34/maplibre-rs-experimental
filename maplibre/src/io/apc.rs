//! Tile worker calls, return messages, and scheduler-backed delivery.

use std::{
    any::{type_name, Any, TypeId},
    fmt::Debug,
    future::Future,
    pin::Pin,
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    coords::WorldTileCoords, define_label, environment::OffscreenKernel,
    io::scheduler::ScheduleError, style::Style,
};

mod attempt;
mod scheduler;
pub(crate) use attempt::AttemptContext;
pub use scheduler::{SchedulerAsyncProcedureCall, SchedulerContext};

define_label!(MessageTag);

impl MessageTag for u32 {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

/// A worker payload has a different Rust type from the one its consumer requested.
#[derive(Error, Debug)]
#[error("message {tag:?} carries payload type {actual:?}, expected {expected}")]
pub struct MessageError {
    /// Routing tag attached to the rejected result.
    pub tag: &'static dyn MessageTag,
    /// Rust type name requested by the consumer.
    pub expected: &'static str,
    /// Runtime type identifier of the rejected payload.
    pub actual: TypeId,
}

/// A tagged worker result with an owned, type-erased payload.
///
/// Tags route messages to consumers; a tag does not enforce the payload's Rust type.
#[derive(Debug)]
pub struct Message {
    tag: &'static dyn MessageTag,
    transferable: Box<dyn Any + Send>,
    attempt: Option<u64>,
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    permit: Option<crate::vector::request_system::SharedTilePermit>,
}

impl Message {
    /// Associates a routing tag with an owned payload; the caller must keep their types consistent.
    pub fn new(tag: &'static dyn MessageTag, transferable: Box<dyn Any + Send>) -> Self {
        Self {
            tag,
            transferable,
            attempt: None,
            #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
            permit: None,
        }
    }

    /// Associates this result with the request that produced it, including partial payloads.
    pub fn with_attempt(mut self, attempt: u64) -> Self {
        self.attempt = Some(attempt);
        self
    }

    /// Attaches an unconsumed shared-memory tile permit released when the message is consumed.
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    pub(crate) fn with_shared_permit(
        mut self,
        permit: crate::vector::request_system::SharedTilePermit,
    ) -> Self {
        self.permit = Some(permit);
        self
    }

    /// Takes any attached shared-memory tile permit so the caller can hold backpressure until
    /// GPU upload completes.
    #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
    pub(crate) fn take_shared_permit(
        &mut self,
    ) -> Option<crate::vector::request_system::SharedTilePermit> {
        self.permit.take()
    }

    /// Request identity retained independently of the concrete worker payload.
    pub fn attempt(&self) -> Option<u64> {
        self.attempt
    }

    /// Takes the payload as its concrete Rust type.
    /// A mismatch drops the payload and returns its tag and type context.
    pub fn into_transferable<T: 'static>(self) -> Result<Box<T>, MessageError> {
        let actual = self.transferable.as_ref().type_id();
        let tag = self.tag;
        self.transferable.downcast::<T>().map_err(|_| MessageError {
            tag,
            expected: type_name::<T>(),
            actual,
        })
    }

    /// Tests routing-tag equality, including the tag's concrete type.
    pub fn has_tag(&self, tag: &'static dyn MessageTag) -> bool {
        self.tag == tag
    }

    /// Borrows the underlying payload if it has concrete type `T`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.transferable.downcast_ref::<T>()
    }

    /// Borrows the routing tag for this result.
    pub fn tag(&self) -> &'static dyn MessageTag {
        self.tag
    }
}

/// Encodes a concrete worker result as a tagged message.
pub trait IntoMessage {
    /// Consumes the result, selecting the tag expected by its consumer.
    fn into(self) -> Message;
}

impl IntoMessage for Message {
    fn into(self) -> Message {
        self
    }
}

/// A tile request for an [`AsyncProcedure`], carrying the map-owned attempt its results
/// are matched by.
#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    /// Tile grid location requested from the worker.
    pub coords: WorldTileCoords,
    /// Style snapshot used to choose and process the tile's layers, shared by the requests
    /// made from the same style. It is not serialized: a transport that serializes inputs
    /// hands its workers the style once and sets it on each input they decode.
    #[serde(skip)]
    pub style: Arc<Style>,
    /// Identifier distinguishing requests when coordinates are evicted and reused.
    pub attempt: u64,
    /// The zoom a vector tile is drawn at when that is past its source's last zoom; zero
    /// when it is drawn at its own zoom.
    #[serde(default)]
    pub overscaled_zoom: u8,
    /// Device pixels per layout pixel of the display, which generated images are made for.
    #[serde(default = "one")]
    pub pixel_ratio: f32,
    /// The sources to process, when the request is for a change to their data alone: the
    /// tile keeps what it shows of the others. `None` processes every source.
    #[serde(default)]
    pub only_sources: Option<Vec<String>>,
    /// The tile's data is due again, after a refresh or a retry, so the worker fetches it
    /// anew instead of taking a decode cached by an earlier request.
    #[serde(default)]
    pub refetch: bool,
}

fn one() -> f32 {
    1.0
}

impl Input {
    /// Requests every source of a tile at its own zoom for a display of one pixel per
    /// layout pixel.
    pub fn new(coords: WorldTileCoords, style: impl Into<Arc<Style>>, attempt: u64) -> Self {
        Self {
            coords,
            style: style.into(),
            attempt,
            overscaled_zoom: 0,
            pixel_ratio: 1.0,
            only_sources: None,
            refetch: false,
        }
    }

    /// Parses a serialized worker [`Input`] from UTF-8 JSON bytes without `serde::Deserialize` bloat.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        use crate::util::json::extract::{
            field_err, json_bool_or, json_f64_or, json_opt_string_vec, json_opt_u64, json_opt_u8,
            json_req_coords, missing_field_err,
        };
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let Some(map) = value.as_object() else {
            return Err(field_err("input", "expected object"));
        };
        let attempt = json_opt_u64(map, "attempt")?.ok_or_else(|| missing_field_err("attempt"))?;
        Ok(Self {
            coords: json_req_coords(map, "coords")?,
            style: Arc::new(Style::empty()),
            attempt,
            overscaled_zoom: json_opt_u8(map, "overscaled_zoom")?.unwrap_or(0),
            pixel_ratio: json_f64_or(map, "pixel_ratio", 1.0)? as f32,
            only_sources: json_opt_string_vec(map, "only_sources")?,
            refetch: json_bool_or(map, "refetch", false)?,
        })
    }
}

/// A worker result could not be delivered to its caller.
#[derive(Error, Debug)]
pub enum SendError {
    /// The transport rejected the message or its receiving endpoint is gone.
    #[error("worker transport failed while {operation}")]
    Transmission {
        /// Transport operation that rejected the result.
        operation: &'static str,
        /// Underlying channel or platform error.
        #[source]
        source: Box<dyn std::error::Error + Send>,
    },
    /// The result has a routing tag unsupported by this transport.
    #[error("worker transport does not support message tag {tag:?}")]
    UnsupportedTag {
        /// Rejected routing tag.
        tag: &'static dyn MessageTag,
    },
    /// The result's payload is incompatible with the transport.
    #[error("worker result has an invalid payload")]
    Payload(#[from] MessageError),
    /// The call the result belongs to was cancelled; the work producing it can stop.
    #[error("the call this result belongs to was cancelled")]
    Abandoned,
}

/// Allows sending messages from workers to back to the caller.
pub trait Context: 'static {
    /// Send a message back to the caller.
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError>;
}

/// Failure during a scheduled procedure, after admission succeeds.
#[derive(Error, Debug)]
pub enum ProcedureError {
    /// Source loading or processing failed.
    #[error("execution of procedure failed")]
    Execution(#[source] Box<dyn std::error::Error>),
    /// A produced result could not be sent back.
    #[error("sending data failed")]
    Send(#[source] SendError),
}

/// Owned procedure execution; transferable between threads in this feature configuration.
#[cfg(feature = "thread-safe-futures")]
pub type AsyncProcedureFuture =
    Pin<Box<dyn Future<Output = Result<(), ProcedureError>> + Send + 'static>>;
/// Owned procedure execution that can retain thread-local state.
#[cfg(not(feature = "thread-safe-futures"))]
pub type AsyncProcedureFuture = Pin<Box<dyn Future<Output = Result<(), ProcedureError>> + 'static>>;

/// A procedure call or incoming worker message could not be prepared or admitted.
#[derive(Error, Debug)]
pub enum CallError {
    /// The executor rejected the call before execution.
    #[error("scheduling work failed")]
    Schedule(#[from] ScheduleError),
    /// The call input could not be encoded.
    #[error("serializing data failed")]
    Serialize(#[source] Box<dyn std::error::Error>),
    /// A worker message could not be decoded.
    #[error("deserializing failed")]
    Deserialize(#[source] Box<dyn std::error::Error>),
    /// A worker could not decode its call input.
    #[error("deserializing input failed")]
    DeserializeInput(#[source] Box<dyn std::error::Error>),
}

/// A statically addressable worker procedure taking owned input, a reply context, and a kernel.
/// Function pointers allow worker transports to identify procedures without serializing closures.
pub type AsyncProcedure<K, C> = fn(input: Input, context: C, kernel: K) -> AsyncProcedureFuture;

/// Wire-safe identifier for offscreen tile procedures across split WASM modules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcedureTag {
    /// Vector tile fetch, decode, tessellation, and symbol layout.
    FetchVector = 1,
    /// Raster image tile fetch and decode.
    FetchRaster = 2,
    /// Terrain / hillshade DEM tile fetch and elevation decode.
    FetchDem = 3,
}

impl ProcedureTag {
    /// Wire byte representation for single-threaded browser worker dispatch.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Decodes a wire byte into its [`ProcedureTag`], or `None` when unrecognized.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::FetchVector),
            2 => Some(Self::FetchRaster),
            3 => Some(Self::FetchDem),
            _ => None,
        }
    }
}

/// Schedules tile work and receives its tagged results without blocking the render loop.
/// Native implementations can transfer owned Rust values; browser workers can serialize them.
pub trait AsyncProcedureCall<K: OffscreenKernel>: 'static {
    /// Cloneable reply endpoint handed to each procedure.
    type Context: Context + Send + Clone;

    /// Owned matching results drained from the transport.
    type ReceiveIterator<F: FnMut(&Message) -> bool>: Iterator<Item = Message>;

    /// Drains currently available matching results, retaining nonmatching messages for other consumers.
    fn receive<F: FnMut(&Message) -> bool>(&self, filter: F) -> Self::ReceiveIterator<F>;

    /// Admits a call without waiting for completion; input or scheduling failures are returned.
    /// Procedure failures occur asynchronously and are handled by the selected implementation.
    fn call(
        &self,
        input: Input,
        procedure: AsyncProcedure<K, Self::Context>,
    ) -> Result<(), CallError>;

    /// Admits a call identified by [`ProcedureTag`] so split single-threaded WASM workers can
    /// dispatch without sharing function-table indices with the renderer module.
    fn call_tagged(
        &self,
        _tag: ProcedureTag,
        input: Input,
        procedure: AsyncProcedure<K, Self::Context>,
    ) -> Result<(), CallError> {
        self.call(input, procedure)
    }

    /// Whether this transport supports dispatching initial tile fetches before Frame 1 while GPU
    /// initialization is still in flight.
    fn supports_prewarm(&self) -> bool {
        false
    }

    /// Dispatches a prewarm tile call by [`ProcedureTag`] while the renderer is initializing.
    fn prewarm_call(&self, _tag: ProcedureTag, _input: Input) -> Result<(), CallError> {
        Ok(())
    }

    /// Stops the tracked call with `attempt` if it is still running. A transport that cannot
    /// reach its worker lets the call finish; the map ignores a result it gave up on.
    fn cancel(&self, _attempt: u64) {}

    /// Stops a batch of cancelled `attempts` in one transport pass when possible.
    fn cancel_batch(&self, attempts: &[u64]) {
        for &attempt in attempts {
            self.cancel(attempt);
        }
    }

    /// Whether results arrived that no [`Self::receive`] has offered to its consumers yet, so
    /// the next frame has something to apply. A transport that cannot tell answers `false`.
    ///
    /// Any receive counts as offering, which holds because every consumer receives in every
    /// frame; ask between frames.
    fn has_arrivals(&self) -> bool {
        false
    }

    /// The image providers the workers ask, when they share this transport's memory. A
    /// transport whose workers build their own configuration has none to offer here.
    fn image_providers(&self) -> Option<&crate::sdf::assets::ImageProviders> {
        None
    }

    /// Whether this transport notifies a registered waker when worker results arrive.
    fn supports_waker(&self) -> bool {
        false
    }

    /// Registers a callback invoked when a worker delivers results to this transport.
    fn set_waker(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}

    /// Pre-delivers `style` to offscreen workers while GPU initialization is still in flight so
    /// workers can parse the style and warm symbol assets concurrently.
    fn warm_style(&self, _style: &Style) {}

    /// Number of dispatched worker calls still in flight (default `0`).
    fn in_flight_calls(&self) -> usize {
        0
    }

    /// Controls whether `receive` lifts steady-state per-frame batch caps during initial load.
    fn set_boot_unthrottled(&self, _unthrottled: bool) {}
}

/// Applies valid results in the entire drained batch before reporting its first type error.
pub(crate) fn apply_worker_messages(
    messages: impl IntoIterator<Item = Message>,
    mut apply: impl FnMut(Message) -> Result<(), MessageError>,
) -> Result<(), MessageError> {
    let mut first_error = None;
    for message in messages {
        if let Err(error) = apply(message) {
            tracing::error!(%error, "worker result rejected");
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
pub(crate) mod tests;
