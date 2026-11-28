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

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
fn run_dispatch_case(track_in_running: bool, abort_early: bool, send_nothing: bool) -> usize {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    };

    use futures::{executor::block_on, future::AbortHandle};

    use super::dispatch::{run_tracked_call, CallDispatch, RunningCalls};
    use crate::{
        coords::{WorldTileCoords, ZoomLevel},
        environment::OffscreenKernelConfig,
        io::apc::{AsyncProcedure, Input},
        platform::ReqwestOffscreenKernelEnvironment,
        style::Style,
    };

    let (sender, _rx) = mpsc::channel();
    let (batch_sender, _batch_rx) = mpsc::channel();
    let wake_count = Arc::new(AtomicUsize::new(0));
    let waker_hits = wake_count.clone();
    let (abort, registration) = AbortHandle::new_pair();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let sent_any = Arc::new(AtomicBool::new(!send_nothing));
    let running: RunningCalls = Arc::default();
    let attempt = 42;
    if track_in_running {
        running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(attempt, (abort, cancel_flag.clone()));
    } else if abort_early {
        abort.abort();
    }
    let in_flight = Arc::new(AtomicUsize::new(usize::from(track_in_running)));
    let active_calls = Arc::new(AtomicUsize::new(1));
    let procedure: AsyncProcedure<ReqwestOffscreenKernelEnvironment, super::SchedulerContext> =
        if abort_early {
            |_input, _context, _kernel| {
                Box::pin(async {
                    futures::future::pending::<()>().await;
                    Ok(())
                })
            }
        } else {
            |_input, _context, _kernel| Box::pin(async { Ok(()) })
        };

    let dispatch = CallDispatch::<ReqwestOffscreenKernelEnvironment> {
        input: Input::new(
            WorldTileCoords::from((0, 0, ZoomLevel::new(0))),
            Arc::new(Style::default()),
            attempt,
        ),
        procedure,
        config: OffscreenKernelConfig::default(),
        sender,
        batch_sender,
        waker: Some(Arc::new(move || {
            waker_hits.fetch_add(1, Ordering::SeqCst);
        })),
        registration,
        cancel_flag,
        sent_any,
        running: running.clone(),
        in_flight: in_flight.clone(),
        active_calls: active_calls.clone(),
    };
    block_on(run_tracked_call(dispatch));

    assert_eq!(active_calls.load(Ordering::SeqCst), 0);
    assert_eq!(in_flight.load(Ordering::SeqCst), 0);
    assert!(running.lock().unwrap_or_else(|p| p.into_inner()).is_empty());
    wake_count.load(Ordering::SeqCst)
}

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
fn cancelled_call_completion_does_not_trigger_idle_wake() {
    // Simulate `cancel(attempt)` having already removed the attempt and aborted its handle.
    assert_eq!(run_dispatch_case(false, true, true), 0);
}

#[cfg(all(feature = "native-io", not(target_arch = "wasm32")))]
#[test]
fn tracked_empty_call_completion_triggers_idle_wake() {
    assert_eq!(run_dispatch_case(true, false, true), 1);
}
