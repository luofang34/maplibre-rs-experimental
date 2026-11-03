use futures::future::{pending, Abortable};
use wasm_bindgen_test::wasm_bindgen_test;

use super::RunningCalls;

#[wasm_bindgen_test]
fn cancelling_a_running_call_aborts_only_that_call() {
    let mut calls = RunningCalls::default();
    let first = Abortable::new(pending::<()>(), calls.start(1));
    let second = Abortable::new(pending::<()>(), calls.start(2));
    assert!(calls.cancel(1));
    assert!(first.is_aborted());
    assert!(!second.is_aborted());
    assert!(!calls.cancel(1), "a cancelled call is no longer running");
}

#[wasm_bindgen_test]
fn a_finished_call_cannot_be_cancelled() {
    let mut calls = RunningCalls::default();
    let call = Abortable::new(pending::<()>(), calls.start(3));
    calls.finish(3);
    assert!(!calls.cancel(3));
    assert!(!call.is_aborted());
}
