use wasm_bindgen_test::wasm_bindgen_test;

use super::Assignments;

#[wasm_bindgen_test]
fn a_cancel_reaches_the_worker_that_received_the_call_once() {
    let mut assignments = Assignments::default();
    assignments.assign(4, 2);
    assignments.assign(5, 0);
    assert_eq!(assignments.cancel(4), Some(2));
    assert_eq!(assignments.cancel(4), None, "the worker was already told");
    assert_eq!(assignments.len(), 1, "the other call is still running");
}

#[wasm_bindgen_test]
fn a_finished_call_is_neither_cancelled_nor_retained() {
    let mut assignments = Assignments::default();
    assignments.assign(4, 1);
    assignments.finished(4);
    assert_eq!(assignments.cancel(4), None);
    assert_eq!(assignments.len(), 0);
}
