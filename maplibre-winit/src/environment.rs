//! Platform types connecting the renderer to winit.

use std::marker::PhantomData;

use maplibre::{
    environment::{Environment, OffscreenKernel},
    io::{apc::AsyncProcedureCall, scheduler::Scheduler, source_client::HttpClient},
};

use crate::WinitMapWindowConfig;

pub struct WinitEnvironment<
    S: Scheduler,
    HC: HttpClient,
    K: OffscreenKernel,
    APC: AsyncProcedureCall<K>,
    ET,
> {
    phantom_s: PhantomData<S>,
    phantom_hc: PhantomData<HC>,
    phantom_k: PhantomData<K>,
    phantom_apc: PhantomData<APC>,
    phantom_et: PhantomData<ET>,
}

impl<
        S: Scheduler,
        HC: HttpClient,
        K: OffscreenKernel,
        APC: AsyncProcedureCall<K>,
        ET: 'static + Clone,
    > Environment for WinitEnvironment<S, HC, K, APC, ET>
{
    type MapWindowConfig = WinitMapWindowConfig<ET>;
    type AsyncProcedureCall = APC;
    type Scheduler = S;
    type HttpClient = HC;
    type OffscreenKernelEnvironment = K;
}
