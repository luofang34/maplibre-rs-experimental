#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
fn cancel_and_cancel_batch_purge_local_queued_and_buffered_messages() {
    use super::*;
    use crate::{
        environment::OffscreenKernelConfig, io::scheduler::NopScheduler,
        platform::ReqwestOffscreenKernelEnvironment,
    };

    let apc = SchedulerAsyncProcedureCall::<ReqwestOffscreenKernelEnvironment, _>::new(
        NopScheduler,
        OffscreenKernelConfig::default(),
    );

    apc.queued
        .borrow_mut()
        .push_back(Message::new(&0_u32, Box::new(10_u32)).with_attempt(1));
    apc.queued
        .borrow_mut()
        .push_back(Message::new(&0_u32, Box::new(20_u32)).with_attempt(2));
    apc.queued
        .borrow_mut()
        .push_back(Message::new(&0_u32, Box::new(30_u32)));
    apc.buffer
        .borrow_mut()
        .push(Message::new(&0_u32, Box::new(40_u32)).with_attempt(1));
    apc.buffer
        .borrow_mut()
        .push(Message::new(&0_u32, Box::new(50_u32)).with_attempt(3));

    apc.cancel(1);
    assert_eq!(apc.queued.borrow().len(), 2);
    assert_eq!(apc.buffer.borrow().len(), 1);
    assert_eq!(apc.cancelled_len(), 1);

    apc.queued
        .borrow_mut()
        .push_back(Message::new(&0_u32, Box::new(60_u32)).with_attempt(4));
    apc.cancel_batch(&[2, 4]);
    assert_eq!(apc.queued.borrow().len(), 1);
    assert_eq!(apc.buffer.borrow().len(), 1);

    let delivered: Vec<u32> = apc
        .receive(|_| true)
        .filter_map(|msg| msg.into_transferable::<u32>().ok().map(|b| *b))
        .collect();
    assert_eq!(delivered, [50, 30]);
}

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
fn receive_skips_pre_cancelled_messages_remaining_in_queued() {
    use super::*;
    use crate::{
        environment::OffscreenKernelConfig, io::scheduler::NopScheduler,
        platform::ReqwestOffscreenKernelEnvironment,
    };

    let apc = SchedulerAsyncProcedureCall::<ReqwestOffscreenKernelEnvironment, _>::new(
        NopScheduler,
        OffscreenKernelConfig::default(),
    );
    apc.cancelled.borrow_mut().insert(99);
    apc.queued
        .borrow_mut()
        .push_back(Message::new(&0_u32, Box::new(111_u32)).with_attempt(99));
    apc.queued
        .borrow_mut()
        .push_back(Message::new(&0_u32, Box::new(222_u32)).with_attempt(100));

    let delivered: Vec<u32> = apc
        .receive(|_| true)
        .filter_map(|msg| msg.into_transferable::<u32>().ok().map(|b| *b))
        .collect();
    assert_eq!(delivered, [222]);
    assert!(apc.queued.borrow().is_empty());
}
