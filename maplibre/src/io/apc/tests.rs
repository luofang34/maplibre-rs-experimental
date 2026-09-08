use crate::io::apc::{Context, IntoMessage, SendError};

pub struct DummyContext;

impl Context for DummyContext {
    fn send_back<T: IntoMessage>(&self, _message: T) -> Result<(), SendError> {
        Ok(())
    }
}
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[allow(clippy::expect_used)]
fn buffered_completion_messages_keep_fifo_order_and_drain_in_one_receive() {
    use super::{AsyncProcedureCall, Message, SchedulerAsyncProcedureCall, SchedulerContext};
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
        .map(|message| *message.into_transferable::<u32>())
        .collect();
    assert_eq!(retained, [7], "other consumers must retain their messages");
}
