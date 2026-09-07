#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use super::Resources;

struct Counted {
    drops: Arc<AtomicUsize>,
    value: u32,
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn inserting_a_resource_again_replaces_and_drops_the_one_before() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut resources = Resources::default();
    for value in 0..100 {
        resources.insert(Counted {
            drops: Arc::clone(&drops),
            value,
        });
    }
    assert_eq!(
        drops.load(Ordering::SeqCst),
        99,
        "every replaced resource is dropped"
    );
    assert_eq!(resources.get::<Counted>().expect("the resource").value, 99);
    assert_eq!(
        resources.resources.len(),
        1,
        "a replaced resource takes no new slot"
    );
}
