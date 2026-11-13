//! Per-world heap byte sampler so frame memory growth can be charged to individual systems
//! without global mutable state in `maplibre`.

use crate::tcs::resources::Resources;

/// Resource backed by a host-supplied function returning the process's current live heap bytes.
///
/// Hosts that install a counting allocator insert a [`HeapSampler`] into [`Resources`]; worlds
/// without one report `0` from [`live_bytes`].
#[derive(Clone, Copy)]
pub struct HeapSampler(fn() -> isize);

impl HeapSampler {
    /// Wraps `read` so system timing and residency summaries can sample live heap bytes.
    pub const fn new(read: fn() -> isize) -> Self {
        Self(read)
    }

    /// Reads the current live heap byte count from the host sampler.
    pub fn sample(self) -> isize {
        (self.0)()
    }
}

/// Live heap bytes reported by the [`HeapSampler`] installed in `resources`, or `0` if absent.
pub fn live_bytes(resources: &Resources) -> isize {
    resources
        .get::<HeapSampler>()
        .copied()
        .map_or(0, HeapSampler::sample)
}

#[cfg(test)]
mod tests {
    use super::{live_bytes, HeapSampler};
    use crate::tcs::resources::Resources;

    #[test]
    fn live_bytes_defaults_to_zero_until_a_sampler_resource_is_installed() {
        fn sample_constant() -> isize {
            42_496
        }

        let mut resources = Resources::default();
        assert_eq!(live_bytes(&resources), 0);
        resources.insert(HeapSampler::new(sample_constant));
        assert_eq!(live_bytes(&resources), 42_496);
    }
}
