//! C ABI of the maplibre-rs renderer for the visionOS host app.
//!
//! The host owns the compositor. Per frame it says where the map's scene stands in its world
//! and, per eye, where the eye is and what it sees. The map draws the eye into its own Metal
//! texture and returns it for the host to copy into the compositor's drawable, and writes
//! the eye's depth straight into the compositor's depth texture.

// C and Metal interop require explicit raw-pointer contracts.
#![allow(unsafe_code)]

mod api;
mod celestial;
mod images;
mod selection;
pub use api::*;
pub use celestial::*;
pub use images::*;
pub use selection::{maplibre_visionos_query_symbols, maplibre_visionos_set_opaque_environment};

mod symbols;
mod trace;
pub use trace::*;

// Compile-time guard: `maplibre-visionos` MUST link `maplibre` with `native-io` enabled so
// `HeadlessEnvironment` schedules vector, raster, and DEM tile requests on `TokioScheduler`
// rather than rejecting every tile via `NopScheduler`.
const _: fn() = || {
    fn assert_tokio_apc<T>()
    where
        T: maplibre::environment::Environment<
            AsyncProcedureCall = maplibre::io::apc::SchedulerAsyncProcedureCall<
                maplibre::platform::ReqwestOffscreenKernelEnvironment,
                maplibre::platform::scheduler::TokioScheduler,
            >,
        >,
    {
    }
    assert_tokio_apc::<maplibre::headless::environment::HeadlessEnvironment>();
};
