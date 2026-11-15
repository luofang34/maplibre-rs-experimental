//! The style each worker holds, so a call names it instead of carrying a copy.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

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

fn json_fingerprint(json: &str) -> (usize, u64, u64) {
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    json.hash(&mut h1);
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    0x9e37_79b9_7f4a_7c15_u64.hash(&mut h2);
    json.as_bytes().hash(&mut h2);
    (json.len(), h1.finish(), h2.finish())
}

/// The last style sent to every worker, and its revision.
#[derive(Default)]
pub(crate) struct StyleRevisions {
    /// The snapshot the revision was last confirmed for; requests of one frame share it.
    snapshot: Option<Arc<Style>>,
    fingerprint: Option<(usize, u64, u64)>,
    revision: u64,
}

impl StyleRevisions {
    /// Pre-sends `style` to workers before an `Arc<Style>` request snapshot exists.
    pub(crate) fn warm<E: std::error::Error + 'static>(
        &mut self,
        style: &Style,
        post: impl FnOnce(u64, &str) -> Result<(), E>,
    ) -> Result<u64, StyleRevisionError<E>> {
        let json = serde_json::to_string(style).map_err(StyleRevisionError::Encode)?;
        let fp = json_fingerprint(&json);
        if self.fingerprint != Some(fp) {
            let revision = self.revision.wrapping_add(1);
            post(revision, &json)
                .map_err(|source| StyleRevisionError::Post { revision, source })?;
            self.revision = revision;
            self.fingerprint = Some(fp);
            self.snapshot = None;
        }
        Ok(self.revision)
    }

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
        let fp = json_fingerprint(&json);
        if self.fingerprint != Some(fp) {
            let revision = self.revision.wrapping_add(1);
            post(revision, &json)
                .map_err(|source| StyleRevisionError::Post { revision, source })?;
            self.revision = revision;
            self.fingerprint = Some(fp);
        }
        drop(json);
        self.snapshot = Some(style.clone());
        Ok(self.revision)
    }
}

#[cfg(test)]
mod tests;
