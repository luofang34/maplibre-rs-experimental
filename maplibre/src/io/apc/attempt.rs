//! Keeps every partial result associated with its admitted tile request.

use super::{Context, IntoMessage, SendError};

#[derive(Clone)]
pub(crate) struct AttemptContext<C> {
    context: C,
    attempt: Option<u64>,
}

impl<C> AttemptContext<C> {
    pub(crate) fn new(context: C, attempt: Option<u64>) -> Self {
        Self { context, attempt }
    }
}

impl<C: Context> Context for AttemptContext<C> {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        let message = message.into();
        let message = match self.attempt {
            Some(attempt) => message.with_attempt(attempt),
            None => message,
        };
        self.context.send_back(message)
    }
}
