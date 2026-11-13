#![allow(clippy::expect_used)]

use std::{cell::Cell, rc::Rc};

use futures::FutureExt;

use super::{MapClosed, Queue, Sender};

/// A queue over a number, with a count of the wakes its sender asked for.
fn queue() -> (Queue<i32>, Sender<i32>, Rc<Cell<u32>>) {
    let commands = Queue::new();
    let wakes = Rc::new(Cell::new(0_u32));
    let counter = wakes.clone();
    let sender = commands.sender(move || counter.set(counter.get().wrapping_add(1)));
    (commands, sender, wakes)
}

#[test]
fn commands_wait_for_the_loop_and_apply_in_the_order_sent() {
    let (commands, sender, _) = queue();
    let mut first = sender.call(|value| {
        *value += 1;
        *value
    });
    let second = sender.call(|value| {
        *value *= 10;
        *value
    });
    assert_eq!((&mut first).now_or_never(), None, "nothing applied yet");

    let mut value = 1;
    commands.apply(&mut value);
    assert_eq!(value, 20);
    assert_eq!(first.now_or_never(), Some(Ok(2)));
    assert_eq!(second.now_or_never(), Some(Ok(20)));
}

#[test]
fn one_wake_reaches_the_loop_per_batch() {
    let (commands, sender, wakes) = queue();
    let replies = [
        sender.call(|_| ()),
        sender.call(|_| ()),
        sender.call(|_| ()),
    ];
    assert_eq!(
        wakes.get(),
        1,
        "the loop drains the whole batch on one wake"
    );

    commands.apply(&mut 0);
    let later = sender.call(|_| ());
    assert_eq!(
        wakes.get(),
        2,
        "a command after a drain wakes the loop again"
    );
    commands.apply(&mut 0);
    for reply in replies.into_iter().chain([later]) {
        assert_eq!(reply.now_or_never(), Some(Ok(())));
    }
}

#[test]
fn closing_rejects_queued_and_later_commands() {
    let (commands, sender, wakes) = queue();
    let queued = sender.call(|value| *value);
    let watching = sender.until(|_| None::<()>);
    commands.close();
    assert!(sender.is_closed());
    assert_eq!(queued.now_or_never(), Some(Err(MapClosed)));
    assert_eq!(watching.now_or_never(), Some(Err(MapClosed)));

    let wakes_before = wakes.get();
    let late = sender.call(|value| *value);
    assert_eq!(late.now_or_never(), Some(Err(MapClosed)));
    assert_eq!(wakes.get(), wakes_before, "a closed loop is not woken");
}

#[test]
fn a_waiting_watcher_is_rejected_when_the_loop_closes() {
    let (commands, sender, _) = queue();
    let mut watching = sender.until(|_| None::<()>);
    commands.apply(&mut 0);
    assert_eq!((&mut watching).now_or_never(), None, "still watching");
    commands.close();
    assert_eq!(watching.now_or_never(), Some(Err(MapClosed)));
}

#[test]
fn a_watcher_answers_after_the_frame_that_satisfies_it() {
    let (commands, sender, _) = queue();
    let asked = Rc::new(Cell::new(0_u32));
    let count = asked.clone();
    let mut reply = sender.until(move |value: &mut i32| {
        count.set(count.get().wrapping_add(1));
        (*value >= 3).then_some(*value)
    });
    let mut value = 0;
    commands.apply(&mut value);
    assert_eq!(asked.get(), 1, "asked once when applied");
    for frame in 1..=3 {
        value = frame;
        commands.observe(&mut value);
    }
    assert_eq!((&mut reply).now_or_never(), Some(Ok(3)));
    commands.observe(&mut value);
    assert_eq!(asked.get(), 4, "an answered watcher is not asked again");
}

#[test]
fn a_watcher_satisfied_at_once_never_waits_for_a_frame() {
    let (commands, sender, _) = queue();
    let reply = sender.until(|value: &mut i32| Some(*value));
    commands.apply(&mut 7);
    assert_eq!(reply.now_or_never(), Some(Ok(7)));
}

#[test]
fn a_command_may_send_another_which_runs_in_the_same_drain() {
    let (commands, sender, _) = queue();
    let inner = sender.clone();
    let nested = Rc::new(Cell::new(None));
    let slot = nested.clone();
    let outer = sender.call(move |value: &mut i32| {
        *value += 1;
        slot.set(Some(inner.call(|value: &mut i32| *value * 100)));
    });
    commands.apply(&mut 1);
    assert_eq!(outer.now_or_never(), Some(Ok(())));
    let nested = nested.take().expect("the outer command sent one");
    assert_eq!(nested.now_or_never(), Some(Ok(200)));
}

#[test]
fn a_dropped_reply_still_applies_its_command() {
    let (commands, sender, _) = queue();
    drop(sender.call(|value: &mut i32| *value = 5));
    let mut value = 0;
    commands.apply(&mut value);
    assert_eq!(value, 5);
}

#[test]
fn sync_handles_are_send_and_sync_even_for_non_send_targets() {
    fn assert_send_sync<T: Send + Sync + Clone + 'static>() {}
    assert_send_sync::<super::SyncHandle<Rc<Cell<i32>>>>();
    #[cfg(feature = "headless")]
    assert_send_sync::<super::SyncHeadlessHandle>();
}

#[test]
fn sync_queue_never_holds_mutex_while_running_or_dropping_closures() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct QueryOnDrop {
        handle: super::SyncHandle<i32>,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for QueryOnDrop {
        fn drop(&mut self) {
            // Re-acquires the queue mutex inside Drop; would deadlock if close() or push()
            // dropped closures while holding the mutex.
            let _ = self.handle.is_closed();
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    let queue = super::SyncCommandQueue::<i32>::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake_counter = Arc::clone(&wakes);
    let handle = queue.handle(move || {
        wake_counter.fetch_add(1, Ordering::SeqCst);
    });

    // Re-entrant send while running apply().
    let nested_handle = handle.clone();
    let outer = handle.call(move |target| {
        *target += 3;
        nested_handle.send_fire_and_forget(|inner| *inner *= 4);
        *target
    });
    let mut state = 2;
    queue.apply(&mut state);
    assert_eq!(outer.now_or_never(), Some(Ok(5)));
    assert_eq!(state, 20);

    // Closure + watcher drop outside mutex on close() and on post-close push().
    let drops = Arc::new(AtomicUsize::new(0));
    let guard_watch = QueryOnDrop {
        handle: handle.clone(),
        drops: Arc::clone(&drops),
    };
    let watching = handle.until(move |_| {
        let _keep = &guard_watch;
        None::<i32>
    });
    queue.apply(&mut state);

    let guard_queued = QueryOnDrop {
        handle: handle.clone(),
        drops: Arc::clone(&drops),
    };
    let queued = handle.call(move |target| {
        let _keep = &guard_queued;
        *target
    });
    queue.close();
    assert_eq!(queued.now_or_never(), Some(Err(MapClosed)));
    assert_eq!(watching.now_or_never(), Some(Err(MapClosed)));

    let guard_late = QueryOnDrop {
        handle: handle.clone(),
        drops: Arc::clone(&drops),
    };
    let late = handle.call(move |target| {
        let _keep = &guard_late;
        *target
    });
    assert_eq!(late.now_or_never(), Some(Err(MapClosed)));
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}

#[test]
fn sync_queue_processes_concurrent_multithreaded_calls_and_watchers() {
    use std::{
        sync::{Arc, Condvar, Mutex},
        thread,
    };

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let wake_pair = Arc::clone(&pair);
    let queue = Arc::new(super::SyncCommandQueue::<i32>::new());
    let handle = queue.handle(move || {
        let (lock, cvar) = &*wake_pair;
        if let Ok(mut woken) = lock.lock() {
            *woken = true;
            cvar.notify_one();
        }
    });

    const THREADS: i32 = 6;
    const OPS_PER_THREAD: i32 = 25;
    let total_expected = THREADS * OPS_PER_THREAD;

    let mut worker_joins = Vec::new();
    for t in 1..=THREADS {
        let h = handle.clone();
        worker_joins.push(thread::spawn(move || {
            let threshold = t * OPS_PER_THREAD;
            let watcher = h.until(move |val| (*val >= threshold).then_some(*val));
            for _ in 0..OPS_PER_THREAD {
                let updated = h.call_blocking(|val| {
                    *val += 1;
                    *val
                });
                assert!(updated.is_ok());
            }
            let reached = futures::executor::block_on(watcher).expect("watcher satisfied");
            assert!(reached >= threshold);
        }));
    }

    let late_watcher = handle.until(move |val| (*val > total_expected + 100).then_some(*val));

    let owner_queue = Arc::clone(&queue);
    let owner_pair = Arc::clone(&pair);
    let owner = thread::spawn(move || {
        let mut state = 0_i32;
        while state < total_expected {
            owner_queue.apply(&mut state);
            if state >= total_expected {
                break;
            }
            let (lock, cvar) = &*owner_pair;
            let mut woken = lock.lock().expect("lock");
            if !*woken && !owner_queue.has_pending_commands() {
                woken = cvar.wait(woken).expect("wait");
            }
            *woken = false;
        }
        owner_queue.apply(&mut state);
        owner_queue.close();
        state
    });

    for join in worker_joins {
        join.join().expect("worker thread panicked");
    }
    let final_state = owner.join().expect("owner thread panicked");
    assert_eq!(final_state, total_expected);
    assert_eq!(
        futures::executor::block_on(late_watcher),
        Err(MapClosed),
        "unsatisfied watcher resolves to MapClosed on shutdown"
    );
    assert_eq!(handle.call_blocking(|v| *v), Err(MapClosed));
}

#[cfg(all(
    feature = "headless",
    feature = "native-io",
    not(target_arch = "wasm32")
))]
#[test]
fn headless_runtime_thread_drives_map_across_os_threads_and_closes_cleanly() {
    use std::thread;

    use cgmath::Deg;

    use crate::{
        coords::LatLon,
        map::handle::{HeadlessRuntimeConfig, HeadlessRuntimeThread},
        render::view_state::CameraOptions,
        style::Style,
    };

    let style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "layers": [
            {
                "id": "bg",
                "type": "background",
                "paint": { "background-color": "#112233" }
            }
        ]
    }))
    .expect("valid style");

    let runtime_thread = HeadlessRuntimeThread::spawn(HeadlessRuntimeConfig {
        width: 32,
        height: 32,
        pixel_ratio: 1.0,
        style,
        cache_dir: None,
        max_pitch_deg: Some(75.0),
        ..HeadlessRuntimeConfig::default()
    })
    .expect("spawn headless runtime thread");

    let handle = runtime_thread.handle();
    let mut joins = Vec::new();
    for idx in 0..4 {
        let thread_handle = handle.clone();
        joins.push(thread::spawn(move || {
            let zoom = 3.0 + f64::from(idx);
            thread_handle
                .call_blocking(move |map| {
                    map.jump_to(CameraOptions {
                        center: Some(LatLon::new(10.0 + f64::from(idx), 20.0)),
                        zoom: Some(zoom),
                        pitch: Some(Deg(15.0)),
                        ..CameraOptions::default()
                    })
                    .expect("jump_to");
                    map.view_state().zoom().value()
                })
                .expect("call succeeded")
        }));
    }
    for join in joins {
        let reported_zoom = join.join().expect("thread join");
        assert!(reported_zoom >= 3.0);
    }

    // Exercise until_blocking watcher waiting for a settled frame on the worker thread.
    let frame_number = handle
        .until_blocking(|map| {
            let stats = map.last_frame_stats();
            (stats.frame >= 1 && !map.needs_redraw()).then_some(stats.frame)
        })
        .expect("settled frame watcher");
    assert!(frame_number >= 1);

    runtime_thread.shutdown();
    assert!(handle.is_closed());
    assert_eq!(handle.call_blocking(|_| ()), Err(MapClosed));
}
