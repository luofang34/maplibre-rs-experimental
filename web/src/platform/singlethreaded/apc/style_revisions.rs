//! The style each worker holds, so a call names it instead of carrying a copy.

use std::sync::Arc;

use maplibre::style::Style;
use thiserror::Error;

/// The style could not be handed to the workers.
#[derive(Debug, Error)]
pub(crate) enum StyleRevisionError<E: std::error::Error + 'static> {
    /// The style could not be encoded.
    #[error("encoding the style for the tile workers failed")]
    Encode(#[source] serde_json::Error),
    /// A worker did not accept the new style.
    #[error("sending style revision {revision} to the tile workers failed")]
    Post {
        /// The revision being sent.
        revision: u64,
        #[source]
        source: E,
    },
}

/// The last style sent to every worker, and its revision.
#[derive(Default)]
pub(crate) struct StyleRevisions {
    /// The snapshot the revision was last confirmed for; requests of one frame share it.
    snapshot: Option<Arc<Style>>,
    json: String,
    revision: u64,
}

impl StyleRevisions {
    /// The revision the workers hold `style` under. A style that differs from the last one sent
    /// is posted to every worker through `post` under a new revision first.
    pub(crate) fn revision<E: std::error::Error + 'static>(
        &mut self,
        style: &Arc<Style>,
        post: impl FnOnce(u64, &str) -> Result<(), E>,
    ) -> Result<u64, StyleRevisionError<E>> {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|sent| Arc::ptr_eq(sent, style))
        {
            return Ok(self.revision);
        }
        // A new snapshot of an unchanged style keeps its revision, so the workers keep theirs.
        let json = serde_json::to_string(&**style).map_err(StyleRevisionError::Encode)?;
        if self.snapshot.is_none() || json != self.json {
            let revision = self.revision.wrapping_add(1);
            post(revision, &json)
                .map_err(|source| StyleRevisionError::Post { revision, source })?;
            self.revision = revision;
            self.json = json;
        }
        self.snapshot = Some(style.clone());
        Ok(self.revision)
    }
}

#[cfg(test)]
mod tests;
