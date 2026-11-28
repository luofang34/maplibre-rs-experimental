#![allow(clippy::expect_used, clippy::panic)]

use super::run_cpu_bound;

#[cfg(feature = "native-io")]
#[tokio::test]
async fn work_runs_off_the_thread_that_awaits_it() {
    let caller = std::thread::current().id();
    let worker = run_cpu_bound(|| std::thread::current().id())
        .await
        .expect("work completes");
    assert_ne!(worker, caller, "the runtime thread stays free for fetches");
}

#[test]
fn work_without_a_runtime_runs_inline() {
    let caller = std::thread::current().id();
    let worker = futures::executor::block_on(run_cpu_bound(|| std::thread::current().id()))
        .expect("work completes");
    assert_eq!(worker, caller);
}

#[cfg(feature = "native-io")]
#[tokio::test]
async fn a_panic_in_the_work_is_an_error() {
    let error = run_cpu_bound(|| -> () { panic!("tessellation bug") })
        .await
        .expect_err("the panic is reported");
    assert!(std::error::Error::source(&error).is_some());
}
