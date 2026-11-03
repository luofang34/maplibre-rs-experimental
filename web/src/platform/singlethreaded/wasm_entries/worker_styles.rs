//! The styles a worker received, by revision, for the calls that name them.

use std::{collections::VecDeque, sync::Arc};

use maplibre::style::Style;

/// Revisions a worker keeps. A call names the latest revision when it is sent, but can start
/// after newer ones arrived while it waited for the worker to be ready.
const KEPT: usize = 8;

/// The most recent styles a worker received, newest last.
#[derive(Default)]
pub(crate) struct WorkerStyles(VecDeque<(u64, Arc<Style>)>);

impl WorkerStyles {
    /// Keeps `style` as `revision`, forgetting the oldest beyond [`KEPT`].
    pub(crate) fn insert(&mut self, revision: u64, style: Style) {
        self.0.retain(|(kept, _)| *kept != revision);
        self.0.push_back((revision, Arc::new(style)));
        while self.0.len() > KEPT {
            self.0.pop_front();
        }
    }

    /// The style received as `revision`, if it is still kept.
    pub(crate) fn get(&self, revision: u64) -> Option<Arc<Style>> {
        self.0
            .iter()
            .rev()
            .find(|(kept, _)| *kept == revision)
            .map(|(_, style)| style.clone())
    }
}

#[cfg(test)]
mod tests;
