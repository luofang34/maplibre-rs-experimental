#![allow(clippy::expect_used, clippy::panic)]

use std::{
    any::{type_name, TypeId},
    error::Error,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
};

use super::super::{apply_worker_messages, Context, IntoMessage, Message, SchedulerContext};

#[test]
fn wrong_payload_is_reported_without_panicking() {
    let outcome = std::panic::catch_unwind(|| {
        Message::new(&7_u32, Box::new(42_u32)).into_transferable::<String>()
    });
    let error = outcome.expect("no unwind").expect_err("wrong payload type");
    assert_eq!(error.expected, type_name::<String>());
    assert_eq!(error.actual, TypeId::of::<u32>());
    assert_eq!(error.tag, &7_u32 as &dyn super::super::MessageTag);
}

#[test]
fn payload_ownership_is_released_once_on_success_and_rejection() {
    #[derive(Debug)]
    struct Payload(Arc<AtomicUsize>);
    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let payload = Message::new(&1_u32, Box::new(Payload(drops.clone())))
        .into_transferable::<Payload>()
        .expect("matching payload");
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(payload);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    {
        let error = Message::new(&1_u32, Box::new(Payload(drops.clone())))
            .into_transferable::<u32>()
            .expect_err("type mismatch");
        assert_eq!(error.actual, TypeId::of::<Payload>());
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn invalid_payload_does_not_discard_later_valid_results_in_the_batch() {
    let messages = vec![
        Message::new(&0_u32, Box::new(10_u32)),
        Message::new(&1_u32, Box::new("bad")),
        Message::new(&0_u32, Box::new(30_u32)),
        Message::new(&2_u32, Box::new(false)),
    ];
    let mut applied = Vec::new();
    let error = apply_worker_messages(messages, |message| {
        applied.push(*message.into_transferable::<u32>()?);
        Ok(())
    })
    .expect_err("batch reports its first error");
    assert_eq!(applied, vec![10, 30]);
    assert_eq!(error.tag, &1_u32 as &dyn super::super::MessageTag);
    assert_eq!(error.actual, TypeId::of::<&str>());
}

#[test]
fn a_disconnected_caller_retains_the_channel_error_and_rejected_result() {
    struct Reply;
    impl IntoMessage for Reply {
        fn into(self) -> Message {
            Message::new(&9_u32, Box::new(42_u32))
        }
    }
    let (sender, receiver) = mpsc::channel();
    drop(receiver);
    let error = SchedulerContext { sender }
        .send_back(Reply)
        .expect_err("receiver disconnected");
    let source = error
        .source()
        .expect("channel cause")
        .downcast_ref::<mpsc::SendError<Message>>()
        .expect("typed channel cause");
    assert!(source.0.has_tag(&9_u32));
}
