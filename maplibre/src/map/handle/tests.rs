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
