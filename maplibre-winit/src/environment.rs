//! Platform types connecting the renderer to winit.

use std::marker::PhantomData;

use maplibre::{
    environment::{Environment, OffscreenKernel},
    io::{apc::AsyncProcedureCall, source_client::HttpClient},
};

use crate::WinitMapWindowConfig;

/// Selects winit windows and the host HTTP client and worker transport for a map.
pub struct WinitEnvironment<HC: HttpClient, K: OffscreenKernel, APC: AsyncProcedureCall<K>, ET> {
    phantom_hc: PhantomData<HC>,
    phantom_k: PhantomData<K>,
    phantom_apc: PhantomData<APC>,
    phantom_et: PhantomData<ET>,
}

impl<HC: HttpClient, K: OffscreenKernel, APC: AsyncProcedureCall<K>, ET: 'static + Clone>
    Environment for WinitEnvironment<HC, K, APC, ET>
{
    type MapWindowConfig = WinitMapWindowConfig<ET>;
    type AsyncProcedureCall = APC;
    type HttpClient = HC;
    type OffscreenKernelEnvironment = K;
}
