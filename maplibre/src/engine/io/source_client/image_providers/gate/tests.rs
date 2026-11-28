#![allow(clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use super::Gate;

#[tokio::test]
async fn a_request_waiting_in_a_task_of_its_own_runs_when_a_slot_frees() {
    let gate = Arc::new(Gate::default());
    let tasks = tokio::task::LocalSet::new();
    tasks
        .run_until(async {
            let first = Gate::enter(&gate, 1, 4).await.expect("a free slot");
            // Tiles wait in tasks of their own, which run only when they are woken.
            let waiter = tokio::task::spawn_local({
                let gate = gate.clone();
                async move { Gate::enter(&gate, 1, 4).await.map(drop).is_ok() }
            });
            while gate.load() != (1, 1) {
                tokio::task::yield_now().await;
            }
            drop(first);
            let entered = tokio::time::timeout(Duration::from_secs(10), waiter)
                .await
                .expect("the waiter is woken when the slot frees")
                .expect("the waiting task");
            assert!(entered);
            assert_eq!(gate.load(), (0, 0));
        })
        .await;
}

#[tokio::test]
async fn a_cancelled_waiter_gives_back_its_place() {
    let gate = Arc::new(Gate::default());
    let first = Gate::enter(&gate, 1, 1).await.expect("a free slot");
    let mut waiting = Box::pin(Gate::enter(&gate, 1, 1));
    assert!(futures::poll!(&mut waiting).is_pending());
    assert_eq!(gate.load(), (1, 1));
    assert!(Gate::enter(&gate, 1, 1).await.is_err(), "the queue is full");
    drop(waiting);
    assert_eq!(gate.load(), (1, 0), "the cancelled waiter left the queue");
    drop(first);
    assert!(Gate::enter(&gate, 1, 1).await.is_ok());
}
