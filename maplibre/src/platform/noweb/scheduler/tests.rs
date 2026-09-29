#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::*;

#[cfg(not(feature = "thread-safe-futures"))]
#[test]
fn unsupported_futures_are_rejected_without_running_the_factory() {
    let called = Arc::new(AtomicBool::new(false));
    let factory_called = called.clone();
    let result = TokioScheduler::new().schedule(move || {
        factory_called.store(true, Ordering::SeqCst);
        async {}
    });
    assert!(matches!(result, Err(ScheduleError::NotImplemented)));
    assert!(!called.load(Ordering::SeqCst));
}

#[cfg(feature = "thread-safe-futures")]
#[test]
fn a_missing_runtime_is_an_error_without_running_the_factory() {
    use std::error::Error;

    let called = Arc::new(AtomicBool::new(false));
    let factory_called = called.clone();
    let result = TokioScheduler::new().schedule(move || {
        factory_called.store(true, Ordering::SeqCst);
        async {}
    });
    let error = result.expect_err("no entered runtime");
    assert!(error.source().is_some());
    assert!(!called.load(Ordering::SeqCst));
}

#[cfg(feature = "thread-safe-futures")]
#[tokio::test]
async fn an_accepted_future_runs_on_the_runtime() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    TokioScheduler::new()
        .schedule(move || async move {
            sender.send(42).expect("receiver is present");
        })
        .expect("runtime accepts task");
    assert_eq!(receiver.await.expect("task completes"), 42);
}
