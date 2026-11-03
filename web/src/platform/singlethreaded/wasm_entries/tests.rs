#![allow(clippy::expect_used, clippy::panic)]

use maplibre::io::apc::AsyncProcedureFuture;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

use super::*;

fn never_finishes(
    _: Input,
    _: UsedContext,
    _: UsedOffscreenKernelEnvironment,
) -> AsyncProcedureFuture {
    Box::pin(futures::future::pending())
}

#[wasm_bindgen_test]
async fn a_call_is_cancellable_as_soon_as_the_worker_starts_it() {
    let mut calls = TileWorkerCalls::new();
    calls
        .set_style(
            1,
            serde_json::to_string(&maplibre::style::Style::default()).expect("style"),
        )
        .expect("style kept");
    let procedure: AsyncProcedure<UsedOffscreenKernelEnvironment, UsedContext> = never_finishes;
    let input = serde_json::to_string(&Input::new(
        Default::default(),
        maplibre::style::Style::default(),
        11,
    ))
    .expect("input");

    let call = calls
        .process(procedure as usize as u32, 1, input)
        .expect("call starts");
    assert!(calls.cancel(11), "the call is running before it is polled");

    JsFuture::from(call).await.expect("a stopped call settles");
    assert!(!calls.cancel(11), "a stopped call is forgotten");
}

#[wasm_bindgen_test]
fn finished_call_payloads_round_trip_and_reject_other_lengths() {
    use crate::platform::singlethreaded::apc::encode_call_finished;

    let attempt = u64::MAX - 3;
    assert_eq!(
        decode_call_finished(&encode_call_finished(attempt)).expect("attempt"),
        attempt
    );
    assert!(decode_call_finished(&js_sys::ArrayBuffer::new(7)).is_err());
}

#[wasm_bindgen_test]
fn a_call_naming_a_style_the_worker_lacks_is_refused() {
    let calls = TileWorkerCalls::new();
    let procedure: AsyncProcedure<UsedOffscreenKernelEnvironment, UsedContext> = never_finishes;
    let input = serde_json::to_string(&Input::new(
        Default::default(),
        maplibre::style::Style::default(),
        12,
    ))
    .expect("input");
    assert!(calls.process(procedure as usize as u32, 5, input).is_err());
    assert!(!calls.cancel(12), "a refused call never runs");
}
