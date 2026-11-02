//! Keeps every partial result associated with its admitted tile request.

use super::{Context, IntoMessage, SendError};

#[derive(Clone)]
pub(crate) struct AttemptContext<C> {
    context: C,
    attempt: u64,
}

impl<C> AttemptContext<C> {
    pub(crate) fn new(context: C, attempt: u64) -> Self {
        Self { context, attempt }
    }
}

impl<C: Context> Context for AttemptContext<C> {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.context
            .send_back(message.into().with_attempt(self.attempt))
    }
}
