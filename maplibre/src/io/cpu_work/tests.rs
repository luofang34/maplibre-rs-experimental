#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{mpsc, Arc, Mutex};

use super::{run_cpu_bound, run_cpu_bound_replying};
use crate::io::apc::{Context, IntoMessage, Message, SendError};

#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<Message>>>);

impl Context for Replies {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.0.lock().expect("replies").push(message.into());
        Ok(())
    }
}

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

#[tokio::test]
async fn a_panic_in_the_work_is_an_error() {
    let error = run_cpu_bound(|| -> () { panic!("tessellation bug") })
        .await
        .expect_err("the panic is reported");
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn work_whose_call_was_dropped_stops_at_its_next_reply() {
    let replies = Replies::default();
    let (started, has_started) = mpsc::channel();
    let (resume, resumed) = mpsc::channel::<()>();
    let (reply, replied) = mpsc::channel();
    let call = tokio::spawn(run_cpu_bound_replying(replies.clone(), move |context| {
        started.send(()).expect("test waits");
        resumed.recv().expect("test resumes");
        reply
            .send(context.send_back(Message::new(&0_u32, Box::new(1_u32))))
            .expect("test reads the reply");
    }));
    tokio::task::spawn_blocking(move || has_started.recv())
        .await
        .expect("join")
        .expect("work started");

    call.abort();
    assert!(call.await.expect_err("aborted").is_cancelled());
    resume.send(()).expect("work waits");

    let result = tokio::task::spawn_blocking(move || replied.recv())
        .await
        .expect("join")
        .expect("work replied");
    assert!(matches!(result, Err(SendError::Abandoned)));
    assert!(replies.0.lock().expect("replies").is_empty());
}

#[tokio::test]
async fn replies_of_wanted_work_are_delivered() {
    let replies = Replies::default();
    run_cpu_bound_replying(replies.clone(), |context| {
        context.send_back(Message::new(&0_u32, Box::new(1_u32)))
    })
    .await
    .expect("work completes")
    .expect("reply delivered");
    assert_eq!(replies.0.lock().expect("replies").len(), 1);
}
