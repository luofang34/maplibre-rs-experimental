#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod admission;
#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod completeness;
#[cfg(all(feature = "raster", not(target_arch = "wasm32")))]
mod completion;
mod errors;
mod payload;
#[cfg(not(target_arch = "wasm32"))]
mod proptests;
#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod retry;
#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod systems;

#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
pub(crate) fn reply_context<K: super::OffscreenKernel, S: crate::io::scheduler::Scheduler>(
    apc: &super::SchedulerAsyncProcedureCall<K, S>,
) -> super::SchedulerContext {
    super::SchedulerContext::new(
        apc.channel.0.clone(),
        crate::platform::lock_unpoisoned(&apc.waker).clone(),
    )
}

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
#[allow(clippy::expect_used)]
fn buffered_completion_messages_keep_fifo_order_and_drain_in_one_receive() {
    use super::{
        AsyncProcedureCall, Context, Message, SchedulerAsyncProcedureCall, SchedulerContext,
    };
    use crate::{
        environment::OffscreenKernelConfig,
        io::scheduler::NopScheduler,
        platform::ReqwestOffscreenKernelEnvironment,
        vector::transferables::{DefaultTileTessellated, TileTessellated},
    };
    let apc = SchedulerAsyncProcedureCall::<ReqwestOffscreenKernelEnvironment, _>::new(
        NopScheduler,
        OffscreenKernelConfig {
            cache_directory: None,
            ..Default::default()
        },
    );
    let sender = SchedulerContext::new(
        apc.channel.0.clone(),
        crate::platform::lock_unpoisoned(&apc.waker).clone(),
    );
    let coords = Default::default();
    sender
        .send_back(DefaultTileTessellated::build_partial(coords))
        .expect("base completion");
    sender
        .send_back(DefaultTileTessellated::build_partial(coords))
        .expect("symbol completion");
    sender
        .send_back(DefaultTileTessellated::build_from(coords))
        .expect("final completion");
    assert_eq!(apc.receive(|_| false).count(), 0);
    apc.channel
        .0
        .send(Message::new(&0_u32, Box::new(7_u32)))
        .expect("other consumer");
    let messages: Vec<_> = apc
        .receive(|message| message.has_tag(DefaultTileTessellated::message_tag()))
        .map(|message| {
            message
                .into_transferable::<DefaultTileTessellated>()
                .expect("matching message payload")
                .pending_symbols()
        })
        .collect();
    assert_eq!(
        messages,
        [true, true, false],
        "final completion must stay last"
    );
    let retained: Vec<_> = apc
        .receive(|_| true)
        .map(|message| {
            *message
                .into_transferable::<u32>()
                .expect("matching message payload")
        })
        .collect();
    assert_eq!(retained, [7], "other consumers must retain their messages");
}

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
#[allow(clippy::expect_used)]
fn arrivals_are_reported_until_a_receive_offers_them() {
    use super::{AsyncProcedureCall, Message, SchedulerAsyncProcedureCall};
    use crate::{
        environment::OffscreenKernelConfig, io::scheduler::NopScheduler,
        platform::ReqwestOffscreenKernelEnvironment,
    };
    let apc = SchedulerAsyncProcedureCall::<ReqwestOffscreenKernelEnvironment, _>::new(
        NopScheduler,
        OffscreenKernelConfig::default(),
    );
    assert!(!apc.has_arrivals(), "nothing was sent");

    apc.channel
        .0
        .send(Message::new(&0_u32, Box::new(7_u32)))
        .expect("send");
    assert!(apc.has_arrivals(), "a result waits in the channel");
    assert!(apc.has_arrivals(), "asking again does not consume it");

    let received: Vec<_> = apc
        .receive(|_| true)
        .map(|message| *message.into_transferable::<u32>().expect("u32 payload"))
        .collect();
    assert_eq!(
        received,
        [7],
        "the message moved by the check is still delivered"
    );
    assert!(!apc.has_arrivals(), "a receive offered it");
}

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
#[allow(clippy::expect_used)]
fn buffered_unmatched_arrivals_keep_reporting_until_matching_consumer_drains_them() {
    use super::{AsyncProcedureCall, Message, SchedulerAsyncProcedureCall};
    use crate::{
        environment::OffscreenKernelConfig, io::scheduler::NopScheduler,
        platform::ReqwestOffscreenKernelEnvironment,
    };
    let apc = SchedulerAsyncProcedureCall::<ReqwestOffscreenKernelEnvironment, _>::new(
        NopScheduler,
        OffscreenKernelConfig::default(),
    );
    apc.channel
        .0
        .send(Message::new(&0_u32, Box::new(99_u32)))
        .expect("send");
    assert!(apc.has_arrivals());
    assert_eq!(apc.receive(|_| false).count(), 0);
    assert!(
        apc.has_arrivals(),
        "unmatched messages parked in self.buffer must keep waking the map until their consumer drains them"
    );
    let drained: Vec<_> = apc
        .receive(|_| true)
        .map(|message| *message.into_transferable::<u32>().expect("u32 payload"))
        .collect();
    assert_eq!(drained, [99]);
    assert!(!apc.has_arrivals());
}

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
#[allow(clippy::expect_used)]
fn cancelled_attempt_stays_suppressed_until_active_call_finishes_and_peek_preserves_batch_gate() {
    use std::sync::{Arc, Mutex};

    use super::{AsyncProcedureCall, Input, Message, SchedulerAsyncProcedureCall};
    use crate::{
        environment::OffscreenKernelConfig,
        io::scheduler::{ScheduleError, Scheduler},
        platform::ReqwestOffscreenKernelEnvironment,
    };

    #[derive(Clone, Default)]
    struct QueuedTestScheduler {
        tasks: Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>,
    }

    impl Scheduler for QueuedTestScheduler {
        #[cfg(feature = "thread-safe-futures")]
        fn schedule<T>(
            &self,
            future_factory: impl (FnOnce() -> T) + Send + 'static,
        ) -> Result<(), ScheduleError>
        where
            T: std::future::Future<Output = ()> + Send + 'static,
        {
            self.tasks
                .lock()
                .expect("tasks lock")
                .push(Box::new(move || {
                    futures::executor::block_on(future_factory());
                }));
            Ok(())
        }

        #[cfg(not(feature = "thread-safe-futures"))]
        fn schedule<T>(
            &self,
            future_factory: impl (FnOnce() -> T) + Send + 'static,
        ) -> Result<(), ScheduleError>
        where
            T: std::future::Future<Output = ()> + 'static,
        {
            self.tasks
                .lock()
                .expect("tasks lock")
                .push(Box::new(move || {
                    futures::executor::block_on(future_factory());
                }));
            Ok(())
        }
    }

    fn noop_proc(
        _input: Input,
        _ctx: super::SchedulerContext,
        _kernel: ReqwestOffscreenKernelEnvironment,
    ) -> super::AsyncProcedureFuture {
        Box::pin(async { Ok(()) })
    }

    let scheduler = QueuedTestScheduler::default();
    let apc = SchedulerAsyncProcedureCall::<ReqwestOffscreenKernelEnvironment, _>::new(
        scheduler.clone(),
        OffscreenKernelConfig::default(),
    );

    let input = Input::new(Default::default(), crate::style::Style::empty(), 42);
    apc.call(input, noop_proc).expect("call scheduled");
    apc.cancel(42);
    assert!(
        apc.running().is_empty(),
        "cancel removes running entry immediately"
    );
    assert_eq!(apc.cancelled_len(), 1);

    apc.set_batch_open_for_test(false);
    apc.channel
        .0
        .send(Message::new(&0_u32, Box::new(77_u32)).with_attempt(42))
        .expect("send late message");
    assert!(
        !apc.has_pending_arrivals(),
        "late message for cancelled attempt must still be dropped while active_calls > 0"
    );
    assert!(
        !apc.batch_open(),
        "has_pending_arrivals must not mutate batch_open"
    );
    assert_eq!(apc.cancelled_len(), 1);

    let tasks: Vec<_> = scheduler
        .tasks
        .lock()
        .expect("tasks lock")
        .drain(..)
        .collect();
    for task in tasks {
        task();
    }

    assert!(!apc.has_pending_arrivals());
    assert_eq!(apc.cancelled_len(), 0);
    assert!(!apc.has_arrivals() && apc.batch_open());
}
