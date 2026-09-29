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

#[test]
fn shared_then_mutable_resource_query_rejects_the_alias() {
    let mut resources = Resources::default();
    resources.insert(10_u32);
    assert!(resources.query_mut::<(&u32, &mut u32)>().is_none());
}

#[test]
fn mutable_then_shared_resource_query_rejects_the_alias() {
    let mut resources = Resources::default();
    resources.insert(10_u32);
    assert!(resources.query_mut::<(&mut u32, &u32)>().is_none());
}

#[test]
fn duplicate_mutable_resource_queries_fail_without_panicking() {
    let mut resources = Resources::default();
    resources.insert(10_u32);
    assert!(resources.query_mut::<(&mut u32, &mut u32)>().is_none());
    *resources.get_mut::<u32>().expect("resource") = 20;
    assert_eq!(resources.get::<u32>(), Some(&20));
}

#[test]
fn disjoint_resource_queries_keep_every_reference_valid() {
    let mut resources = Resources::default();
    resources.insert(1_u8);
    resources.insert(2_u16);
    resources.insert(3_u32);
    resources.insert(4_u64);
    let (a, b, c, d, shared, repeated) = resources
        .query_mut::<(&mut u8, &mut u16, &mut u32, &u64, &u64, &u64)>()
        .expect("disjoint query");
    *a = 10;
    *b = 20;
    *c = 30;
    assert_eq!((*a, *b, *c, *d, *shared, *repeated), (10, 20, 30, 4, 4, 4));
    assert!(std::ptr::eq(d, repeated));
    assert_eq!(
        resources.query::<(&u8, &u16, &u32)>(),
        Some((&10, &20, &30))
    );
}

#[test]
fn a_failed_resource_query_leaves_later_queries_available() {
    let mut resources = Resources::default();
    resources.insert(1_u32);
    assert!(resources.query_mut::<(&mut u32, &u64)>().is_none());
    let (value,) = resources.query_mut::<(&mut u32,)>().expect("next query");
    *value = 2;
    assert_eq!(resources.query::<(&u32, &u32)>(), Some((&2, &2)));
}
