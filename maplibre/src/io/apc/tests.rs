#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod admission;
#[cfg(not(target_arch = "wasm32"))]
mod completion;
mod errors;
mod payload;
#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod retry;
#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
mod systems;

#[cfg(all(feature = "headless", not(target_arch = "wasm32")))]
pub(crate) fn reply_context<K: super::OffscreenKernel, S: super::Scheduler>(
    apc: &super::SchedulerAsyncProcedureCall<K, S>,
) -> super::SchedulerContext {
    super::SchedulerContext {
        sender: apc.channel.0.clone(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
    let sender = SchedulerContext {
        sender: apc.channel.0.clone(),
    };
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

#[cfg(not(target_arch = "wasm32"))]
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
