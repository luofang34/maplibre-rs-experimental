//! Tile worker calls, return messages, and scheduler-backed delivery.

use std::{
    any::Any,
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

define_label!(MessageTag);

impl MessageTag for u32 {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

/// A consumer could not interpret an erased message payload.
#[derive(Error, Debug)]
pub enum MessageError {
    /// Retains a payload that could not be cast to the requested type.
    #[error("the message did not contain the expected data")]
    CastError(Box<dyn Any>),
}

/// A tagged worker result with an owned, type-erased payload.
///
/// Tags route messages to consumers; a tag does not enforce the payload's Rust type.
#[derive(Debug)]
pub struct Message {
    tag: &'static dyn MessageTag,
    transferable: Box<dyn Any + Send>,
}

impl Message {
    /// Associates a routing tag with an owned payload; the caller must keep their types consistent.
    pub fn new(tag: &'static dyn MessageTag, transferable: Box<dyn Any + Send>) -> Self {
        Self { tag, transferable }
    }

    /// Takes the payload as its concrete Rust type.
    ///
    /// # Panics
    /// Panics if the payload is not `T`, even when its routing tag matches the consumer.
    pub fn into_transferable<T: 'static>(self) -> Box<T> {
        self.transferable
            .downcast::<T>()
            .expect("message has wrong tag")
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
}

/// A worker result could not be delivered to its caller.
#[derive(Error, Debug)]
pub enum SendError {
    /// The transport rejected the message or its receiving endpoint is gone.
    #[error("could not transmit data")]
    Transmission,
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
            .map_err(|_e| SendError::Transmission)
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

#[cfg(test)]
pub(crate) mod tests;
