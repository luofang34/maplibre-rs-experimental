//! Tile work on the browser's event loop. Run with `CHROMEDRIVER=<path> cargo test -p maplibre
//! --target wasm32-unknown-unknown --features headless --test web_local_scheduler`.
#![cfg(target_arch = "wasm32")]

use futures::channel::oneshot;
use maplibre::{
    environment::OffscreenKernelConfig,
    headless::environment::LoaderKernel,
    io::{
        apc::{
            AsyncProcedureCall, AsyncProcedureFuture, Context, Input, Message,
            SchedulerAsyncProcedureCall, SchedulerContext,
        },
        scheduler::{LocalScheduler, Scheduler},
    },
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

const ANSWER: u32 = 42;

/// Resolves once every task scheduled before it on this thread has run, since local tasks
/// run in the order they were spawned.
async fn tasks_scheduled_so_far() {
    let (done, finished) = oneshot::channel();
    LocalScheduler
        .schedule(move || async move {
            done.send(()).ok();
        })
        .expect("local scheduling is always admitted");
    finished.await.expect("the marker task runs");
}

#[wasm_bindgen_test]
async fn local_scheduler_runs_a_scheduled_future() {
    let (sender, receiver) = oneshot::channel();
    LocalScheduler
        .schedule(move || async move {
            sender.send(ANSWER).ok();
        })
        .expect("local scheduling is always admitted");

    assert_eq!(receiver.await.expect("the task ran"), ANSWER);
}

fn answer(_input: Input, context: SchedulerContext, _kernel: LoaderKernel) -> AsyncProcedureFuture {
    Box::pin(async move {
        context
            .send_back(Message::new(&ANSWER, Box::new(ANSWER)))
            .map_err(maplibre::io::apc::ProcedureError::Send)
    })
}

#[wasm_bindgen_test]
async fn a_call_delivers_its_message_back_to_the_map_thread() {
    let apc = SchedulerAsyncProcedureCall::<LoaderKernel, _>::new(
        LocalScheduler,
        OffscreenKernelConfig::default(),
    );
    let input = Input::TileRequest {
        coords: Default::default(),
        style: Default::default(),
    };
    apc.call(input, answer).expect("the call is admitted");
    assert!(
        !apc.has_arrivals(),
        "the call runs only once the page yields"
    );

    tasks_scheduled_so_far().await;

    assert!(apc.has_arrivals(), "the result waits for the next frame");
    let delivered: Vec<u32> = apc
        .receive(|message| message.has_tag(&ANSWER))
        .map(|message| *message.into_transferable::<u32>().expect("u32 payload"))
        .collect();
    assert_eq!(delivered, [ANSWER]);
    assert!(!apc.has_arrivals());
}
