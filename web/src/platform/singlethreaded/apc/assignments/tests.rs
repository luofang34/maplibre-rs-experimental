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

#[wasm_bindgen_test]
fn least_loaded_worker_steers_around_busy_workers_and_rotates_ties() {
    let mut assignments = Assignments::default();
    assert_eq!(assignments.least_loaded_worker(4), 0);
    assignments.assign(10, 0);
    assignments.assign(11, 0);
    assignments.assign(12, 1);
    assignments.assign(13, 2);
    // Worker 3 is the only idle worker (load 0).
    assert_eq!(assignments.least_loaded_worker(4), 3);
    assignments.assign(14, 3);
    // Workers 1, 2, 3 have load 1 while worker 0 has load 2 -> never picks worker 0.
    let picked = assignments.least_loaded_worker(4);
    assert_ne!(picked, 0);
    assignments.finished(10);
    assignments.finished(11);
    // Worker 0 is now idle again (load 0).
    assert_eq!(assignments.least_loaded_worker(4), 0);
}
