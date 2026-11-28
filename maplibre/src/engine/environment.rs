//! Platform service types and serializable worker configuration.

#![deny(missing_docs)]

pub use crate::io::apc::{OffscreenKernel, OffscreenKernelConfig};
use crate::{
    io::{apc::AsyncProcedureCall, source_client::HttpClient},
    window::MapWindowConfig,
};

/// Platform services selected together at compile time.
/// The worker transport must support the same offscreen kernel used to fetch source data.
pub trait Environment: 'static {
    /// Factory for the map's host window.
    type MapWindowConfig: MapWindowConfig;

    /// Request/reply transport that runs procedures with the offscreen kernel.
    type AsyncProcedureCall: AsyncProcedureCall<Self::OffscreenKernelEnvironment>;

    /// HTTP implementation used by this environment's source loader.
    type HttpClient: HttpClient;

    /// Services that a worker can create from serialized configuration.
    type OffscreenKernelEnvironment: OffscreenKernel;
}
