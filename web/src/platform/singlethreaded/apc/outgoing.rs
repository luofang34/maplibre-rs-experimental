//! Per-worker outgoing call buffers so multiple tile calls posted in one stage coalesce into
//! a single transferred `Uint32Array` batch message per worker.

#![deny(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use crate::platform::singlethreaded::apc::{
    assignments::Assignments, post, recycle::WorkerEndpoint,
};

pub(super) const WORDS_PER_CALL: usize = 8;

pub(super) struct PackedTileCall {
    pub(super) attempt: u64,
    pub(super) style_revision: u64,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) meta: u32,
    pub(super) pixel_ratio: f32,
}

#[derive(Default)]
pub(super) struct OutgoingCallBatches {
    buckets: Vec<Vec<u32>>,
    pending_count: usize,
}

fn unpack_call(words: &[u32; WORDS_PER_CALL]) -> (u64, u64, i32, i32, u32, f32) {
    let attempt = u64::from(words[0]) | (u64::from(words[1]) << 32);
    let style_revision = u64::from(words[2]) | (u64::from(words[3]) << 32);
    let x = words[4] as i32;
    let y = words[5] as i32;
    let meta = words[6];
    let pixel_ratio = f32::from_bits(words[7]);
    (attempt, style_revision, x, y, meta, pixel_ratio)
}

impl OutgoingCallBatches {
    pub(super) fn push(&mut self, worker_idx: usize, worker_count: usize, call: PackedTileCall) {
        if self.buckets.len() < worker_count {
            self.buckets.resize_with(worker_count, Vec::new);
        }
        let Some(bucket) = self.buckets.get_mut(worker_idx) else {
            return;
        };
        bucket.extend_from_slice(&[
            call.attempt as u32,
            (call.attempt >> 32) as u32,
            call.style_revision as u32,
            (call.style_revision >> 32) as u32,
            call.x as u32,
            call.y as u32,
            call.meta,
            call.pixel_ratio.to_bits(),
        ]);
        self.pending_count += 1;
    }

    pub(super) fn remove_pending(&mut self, worker_idx: usize, attempt: u64) -> bool {
        let Some(bucket) = self.buckets.get_mut(worker_idx) else {
            return false;
        };
        let lo = attempt as u32;
        let hi = (attempt >> 32) as u32;
        let Some(pos) = bucket
            .as_chunks::<WORDS_PER_CALL>()
            .0
            .iter()
            .position(|chunk| chunk[0] == lo && chunk[1] == hi)
        else {
            return false;
        };
        let start = pos * WORDS_PER_CALL;
        bucket.drain(start..start + WORDS_PER_CALL);
        self.pending_count = self.pending_count.saturating_sub(1);
        true
    }

    pub(super) fn flush(&mut self, workers: &[WorkerEndpoint], assignments: &mut Assignments) {
        if self.pending_count == 0 {
            return;
        }
        self.pending_count = 0;
        for (idx, bucket) in self.buckets.iter_mut().enumerate() {
            if bucket.is_empty() {
                continue;
            }
            let Some(worker) = workers.get(idx) else {
                for chunk in bucket.as_chunks::<WORDS_PER_CALL>().0 {
                    assignments.cancel(unpack_call(chunk).0);
                }
                bucket.clear();
                continue;
            };
            if let Some(words) = bucket.first_chunk::<WORDS_PER_CALL>() {
                if bucket.len() == WORDS_PER_CALL {
                    let (attempt, rev, x, y, meta, pixel_ratio) = unpack_call(words);
                    if let Err(err) = post::post_st_tile_call(
                        worker.as_js_value(),
                        attempt,
                        rev,
                        x,
                        y,
                        meta,
                        pixel_ratio,
                    ) {
                        assignments.cancel(attempt);
                        tracing::warn!(attempt, ?err, "posting single ST tile call failed");
                    }
                    bucket.clear();
                    continue;
                }
            }
            if let Err(err) = post::post_st_tile_call_batch(worker.as_js_value(), bucket.as_slice())
            {
                for chunk in bucket.as_chunks::<WORDS_PER_CALL>().0 {
                    assignments.cancel(unpack_call(chunk).0);
                }
                tracing::warn!(worker = idx, ?err, "posting batched ST tile calls failed");
            }
            bucket.clear();
            if bucket.capacity() > WORDS_PER_CALL * 64 {
                bucket.shrink_to(WORDS_PER_CALL * 16);
            }
        }
    }

    pub(super) fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.pending_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_call(attempt: u64, x: i32, y: i32) -> PackedTileCall {
        PackedTileCall {
            attempt,
            style_revision: 0x0102_0304_0506_0708,
            x,
            y,
            meta: 0x0105_0301,
            pixel_ratio: 2.0,
        }
    }

    #[test]
    fn pack_remove_and_unpack_preserve_fields() {
        let mut batches = OutgoingCallBatches::default();
        batches.push(0, 2, sample_call(0xdead_beef_cafe_babe, -12, 34));
        batches.push(0, 2, sample_call(42, 7, -9));
        batches.push(1, 2, sample_call(99, 1, 2));
        assert_eq!(batches.pending_count, 3);
        assert!(batches.remove_pending(0, 42));
        assert!(!batches.remove_pending(0, 42));
        assert_eq!(batches.pending_count, 2);

        let unpacked = batches
            .buckets
            .first()
            .and_then(|b| b.first_chunk::<WORDS_PER_CALL>())
            .map(unpack_call);
        assert_eq!(
            unpacked,
            Some((
                0xdead_beef_cafe_babe,
                0x0102_0304_0506_0708,
                -12,
                34,
                0x0105_0301,
                2.0
            ))
        );

        let mut assignments = Assignments::default();
        batches.flush(&[], &mut assignments);
        assert_eq!(batches.pending_count, 0);
        assert!(batches.buckets.iter().all(Vec::is_empty));
    }
}
