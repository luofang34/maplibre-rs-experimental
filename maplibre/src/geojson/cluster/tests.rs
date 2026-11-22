#![allow(clippy::expect_used)]

use super::*;

fn options() -> ClusterOptions {
    ClusterOptions::new(Some(40.0), Some(4), None, None, None)
}

#[test]
fn points_within_the_radius_merge_at_low_zoom_and_part_at_high_zoom() {
    let properties: [(String, Value); 0] = [];
    let points = [
        ([0.5, 0.5], &properties[..]),
        ([0.5 + 0.0001, 0.5], &properties[..]),
        ([0.9, 0.9], &properties[..]),
    ];
    let clusters = Clusters::new(&points, options());
    let world = [0.0, 0.0, 1.0, 1.0];
    let low = clusters.within(0, world);
    assert_eq!(low.len(), 2, "the two close points merge");
    assert!(low
        .iter()
        .any(|item| matches!(item, Clustered::Cluster(_, _, p)
            if p.iter().any(|(name, value)| name == "point_count" && *value == json!(2)))));
    assert_eq!(
        clusters.within(5, world).len(),
        3,
        "past the cluster zoom nothing merges"
    );
}

#[test]
fn counts_of_a_thousand_or_more_abbreviate() {
    assert_eq!(abbreviated(999), json!(999));
    assert_eq!(abbreviated(1234), json!("1.2k"));
    assert_eq!(abbreviated(12_400), json!("12k"));
}

#[test]
fn cluster_properties_support_shorthand_and_arbitrary_accumulated_reduce_expressions() {
    let prop_defs = json!({
        "sum_short": ["+", ["get", "mag"]],
        "max_legacy": ["max", ["accumulated"], ["get", "mag"]],
        "sum_expr": [["+", ["accumulated"], ["get", "sum_expr"]], ["get", "mag"]],
        "sum_by_field": [["+", ["accumulated"], ["get", "mag"]], ["get", "mag"]],
        "capped_max": [
            ["case", [">", ["get", "capped_max"], ["accumulated"]], ["get", "capped_max"], ["accumulated"]],
            ["min", ["get", "mag"], 8]
        ]
    });
    let opts = ClusterOptions::new(Some(60.0), Some(6), None, Some(&prop_defs), None);
    let p1 = [("mag".to_owned(), json!(3))];
    let p2 = [("mag".to_owned(), json!(10))];
    let p3 = [("mag".to_owned(), json!(5))];
    // p1 and p2 merge at zoom 4, then merge with p3 at zoom 0 (hierarchical multi-level merge!).
    let points = [
        ([0.5, 0.5], &p1[..]),
        ([0.5001, 0.5], &p2[..]),
        ([0.51, 0.5], &p3[..]),
    ];
    let clusters = Clusters::new(&points, opts);
    let items = clusters.within(0, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(items.len(), 1);
    assert!(matches!(items.first(), Some(Clustered::Cluster(..))));
    if let Some(Clustered::Cluster(_, _, cluster_props)) = items.first() {
        let lookup = |key: &str| {
            cluster_props
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .expect("cluster property present")
        };
        assert_eq!(lookup("point_count"), json!(3));
        assert_eq!(lookup("sum_short"), json!(18));
        assert_eq!(lookup("max_legacy"), json!(10));
        assert_eq!(lookup("sum_expr"), json!(18));
        assert_eq!(lookup("sum_by_field"), json!(18));
        assert_eq!(lookup("capped_max"), json!(8));
    }
}
