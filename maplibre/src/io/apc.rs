//! Tile worker calls, return messages, and scheduler-backed delivery.

use std::{
    any::{type_name, Any, TypeId},
    cell::RefCell,
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{
        mpsc,
        mpsc::{Receiver, Sender},
    },
    vec::IntoIter,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    coords::WorldTileCoords,
    define_label,
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::scheduler::{ScheduleError, Scheduler},
    style::Style,
};

mod attempt;
pub(crate) use attempt::AttemptContext;

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
}

impl Message {
    /// Associates a routing tag with an owned payload; the caller must keep their types consistent.
    pub fn new(tag: &'static dyn MessageTag, transferable: Box<dyn Any + Send>) -> Self {
        Self {
            tag,
            transferable,
            attempt: None,
        }
    }

    /// Associates this result with the request that produced it, including partial payloads.
    pub fn with_attempt(mut self, attempt: u64) -> Self {
        self.attempt = Some(attempt);
        self
    }

    /// Request identity retained independently of the concrete worker payload.
    pub fn attempt(&self) -> Option<u64> {
        self.attempt
    }

    /// Takes the payload as its concrete Rust type.
    /// A mismatch drops the payload and returns its tag and type context.
    pub fn into_transferable<T: 'static>(self) -> Result<Box<T>, MessageError> {
        let actual = self.transferable.as_ref().type_id();
        self.transferable.downcast::<T>().map_err(|_| MessageError {
            tag: self.tag,
            expected: type_name::<T>(),
            actual,
        })
    }

    /// Tests routing-tag equality, including the tag's concrete type.
    pub fn has_tag(&self, tag: &'static dyn MessageTag) -> bool {
        self.tag == tag
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

/// Inputs for an [`AsyncProcedure`]
#[derive(Clone, Serialize, Deserialize)]
pub enum Input {
    /// Requests source data and processing for a tile using the supplied style snapshot.
    TileRequest {
        /// Tile grid location requested from the worker.
        coords: WorldTileCoords,
        /// Style snapshot used to choose and process the tile's layers.
        style: Style,
    },
    /// Requests a tile with a map-owned attempt token for final completion matching.
    TrackedTileRequest {
        /// Tile grid location requested from the worker.
        coords: WorldTileCoords,
        /// Style snapshot used to choose and process the tile's layers.
        style: Style,
        /// Identifier distinguishing requests when coordinates are evicted and reused.
        attempt: u64,
        /// The zoom a vector tile is drawn at when that is past its source's last zoom; zero
        /// when it is drawn at its own zoom.
        #[serde(default)]
        overscaled_zoom: u8,
    },
}

impl Input {
    /// Consumes a tile request and its optional completion token.
    pub fn into_tile_request(self) -> (WorldTileCoords, Style, Option<u64>) {
        match self {
            Self::TileRequest { coords, style } => (coords, style, None),
            Self::TrackedTileRequest {
                coords,
                style,
                attempt,
                ..
            } => (coords, style, Some(attempt)),
        }
    }

    /// The zoom the requested tile is magnified to, or zero when it is not magnified.
    pub fn overscaled_zoom(&self) -> u8 {
        match self {
            Self::TileRequest { .. } => 0,
            Self::TrackedTileRequest {
                overscaled_zoom, ..
            } => *overscaled_zoom,
        }
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
        source: Box<dyn std::error::Error>,
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
}

/// Reply sender whose clones share the scheduler-backed APC's receiving channel.
#[derive(Clone)]
pub struct SchedulerContext {
    sender: Sender<Message>,
}

impl Context for SchedulerContext {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.sender
            .send(message.into())
            .map_err(|source| SendError::Transmission {
                operation: "sending a result to the caller",
                source: Box::new(source),
            })
    }
}

/// An APC using a scheduler for execution and an in-process channel for replies.
///
/// Receiving drains available matches in delivery order. Procedure failures are logged;
/// only preparation and admission failures are returned by [`AsyncProcedureCall::call`].
pub struct SchedulerAsyncProcedureCall<K: OffscreenKernel, S: Scheduler> {
    channel: (Sender<Message>, Receiver<Message>),
    buffer: RefCell<Vec<Message>>,
    scheduler: S,
    phantom_k: PhantomData<K>,
    offscreen_kernel_config: OffscreenKernelConfig,
}

impl<K: OffscreenKernel, S: Scheduler> SchedulerAsyncProcedureCall<K, S> {
    /// Creates an empty reply channel and keeps the worker kernel configuration for future calls.
    pub fn new(scheduler: S, offscreen_kernel_config: OffscreenKernelConfig) -> Self {
        Self {
            channel: mpsc::channel(),
            buffer: RefCell::new(Vec::new()),
            phantom_k: PhantomData,
            scheduler,
            offscreen_kernel_config,
        }
    }
}

impl<K: OffscreenKernel, S: Scheduler> AsyncProcedureCall<K> for SchedulerAsyncProcedureCall<K, S> {
    type Context = SchedulerContext;
    type ReceiveIterator<F: FnMut(&Message) -> bool> = IntoIter<Message>;

    fn receive<F: FnMut(&Message) -> bool>(&self, mut filter: F) -> Self::ReceiveIterator<F> {
        let mut buffer = self.buffer.borrow_mut();
        // Partial and final tile completions must retain the worker's delivery order.
        let mut ret: Vec<_> = buffer.extract_if(.., |message| filter(message)).collect();

        while let Ok(message) = self.channel.1.try_recv() {
            tracing::debug!("Data reached main thread: {message:?}");

            if filter(&message) {
                ret.push(message);
            } else {
                buffer.push(message)
            }
        }

        ret.into_iter()
    }

    fn call(
        &self,
        input: Input,
        procedure: AsyncProcedure<K, Self::Context>,
    ) -> Result<(), CallError> {
        let sender = self.channel.0.clone();
        let offscreen_kernel_config = self.offscreen_kernel_config.clone();

        self.scheduler
            .schedule(move || async move {
                tracing::debug!(thread = ?std::thread::current().name(), "processing worker call");

                let kernel = K::create(offscreen_kernel_config);
                // A result that cannot be delivered, because the map already shut down, must
                // not take the worker thread down with it.
                if let Err(error) = procedure(input, SchedulerContext { sender }, kernel).await {
                    tracing::warn!(?error, "procedure failed");
                }
            })
            .map_err(CallError::Schedule)
    }
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
