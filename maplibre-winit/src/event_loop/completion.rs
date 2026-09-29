//! Startup notification holds no borrow while browser callbacks or futures execute.
use crate::WinitApplicationError;
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

struct State<C: std::error::Error + 'static> {
    result: Option<Result<(), WinitApplicationError<C>>>,
    waker: Option<Waker>,
}
pub(super) struct Completion<C: std::error::Error + 'static>(Rc<RefCell<State<C>>>);

pub(super) fn channel<C: std::error::Error + 'static>(
) -> (super::lifecycle::Completion<C>, Completion<C>) {
    let state = Rc::new(RefCell::new(State {
        result: None,
        waker: None,
    }));
    let sender = state.clone();
    let callback = Box::new(move |result| {
        let wake = {
            let mut state = sender.borrow_mut();
            state.result = Some(result);
            state.waker.take()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    });
    (callback, Completion(state))
}
impl<C: std::error::Error + 'static> Future for Completion<C> {
    type Output = Result<(), WinitApplicationError<C>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.0.borrow_mut();
        match state.result.take() {
            Some(result) => Poll::Ready(result),
            None => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
