//! Property tests for [`CollisionGrid`] spatial overlap, clamping, and conservative bounds.

use proptest::prelude::*;

use crate::sdf::collision_grid::{intersects, CollisionGrid};

proptest! {
    #[test]
    fn prop_collision_grid_non_overlap_invariant(
        width in 64.0_f64..=2048.0_f64,
        height in 64.0_f64..=2048.0_f64,
        candidates in proptest::collection::vec(
            (-256.0_f64..=2304.0_f64, -256.0_f64..=2304.0_f64, 1.0_f64..=160.0_f64, 1.0_f64..=160.0_f64),
            1..=48,
        ),
    ) {
        let mut grid = CollisionGrid::new(width, height);
        let mut accepted: Vec<[f64; 4]> = Vec::new();
        for (x, y, w, h) in candidates {
            let rect = [x, y, x + w, y + h];
            let expected = accepted.iter().any(|&other| intersects(rect, other));
            prop_assert_eq!(grid.overlaps(rect), expected);
            if !expected {
                grid.insert(rect);
                accepted.push(rect);
            }
        }
        for (i, &a) in accepted.iter().enumerate() {
            for &b in accepted.iter().skip(i + 1) {
                prop_assert!(!intersects(a, b));
            }
        }
    }
}

proptest! {
    #[test]
    fn prop_collision_grid_self_query_consistent(
        width in 64.0_f64..=2048.0_f64,
        height in 64.0_f64..=2048.0_f64,
        entries in proptest::collection::vec(
            (-128.0_f64..=2176.0_f64, -128.0_f64..=2176.0_f64, 2.0_f64..=120.0_f64, 2.0_f64..=120.0_f64, proptest::bool::ANY),
            1..=24,
        ),
    ) {
        let mut grid = CollisionGrid::new(width, height);
        let mut cooperative_grid = CollisionGrid::new(width, height);
        for (x, y, w, h, cooperative) in entries {
            let rect = [x, y, x + w, y + h];
            grid.insert_as(rect, cooperative);
            prop_assert!(grid.overlaps(rect));
            if !cooperative {
                prop_assert!(grid.overlaps_non_cooperative(rect));
            }
            let inset = [x + 0.25 * w, y + 0.25 * h, x + 0.75 * w, y + 0.75 * h];
            prop_assert!(grid.overlaps(inset));

            cooperative_grid.insert_as(rect, true);
            prop_assert!(cooperative_grid.overlaps(rect));
            prop_assert!(!cooperative_grid.overlaps_non_cooperative(rect));
        }
    }
}

proptest! {
    #[test]
    fn prop_collision_grid_cell_clamping_finite(
        width in 0.0_f64..=4096.0_f64,
        height in 0.0_f64..=4096.0_f64,
        x0 in -1e9_f64..=1e9_f64,
        y0 in -1e9_f64..=1e9_f64,
        w in 1.0_f64..=1e9_f64,
        h in 1.0_f64..=1e9_f64,
    ) {
        let mut grid = CollisionGrid::new(width, height);
        let rect = [x0, y0, x0 + w, y0 + h];
        prop_assert!(rect.iter().all(|v| v.is_finite()));
        prop_assert!(!grid.overlaps(rect));
        grid.insert(rect);
        prop_assert!(grid.overlaps(rect));
        prop_assert!(grid.overlaps_non_cooperative(rect));
    }
}

proptest! {
    #[test]
    fn prop_collision_circle_disjointness(
        cx1 in 0.0_f64..=1024.0_f64,
        cy1 in 0.0_f64..=1024.0_f64,
        r1 in 1.0_f64..=128.0_f64,
        cx2 in 0.0_f64..=1024.0_f64,
        cy2 in 0.0_f64..=1024.0_f64,
        r2 in 1.0_f64..=128.0_f64,
    ) {
        let aabb1 = [cx1 - r1, cy1 - r1, cx1 + r1, cy1 + r1];
        let aabb2 = [cx2 - r2, cy2 - r2, cx2 + r2, cy2 + r2];
        let mut grid = CollisionGrid::new(1024.0, 1024.0);
        grid.insert(aabb1);

        let dist = (cx2 - cx1).hypot(cy2 - cy1);
        let radius_sum = r1 + r2;
        if !grid.overlaps(aabb2) {
            prop_assert!(dist + 1e-9 >= radius_sum);
        }
        if dist + 1e-9 < radius_sum {
            prop_assert!(grid.overlaps(aabb2));
        }
        if (cx2 - cx1).abs() > radius_sum + 1e-9 || (cy2 - cy1).abs() > radius_sum + 1e-9 {
            prop_assert!(!grid.overlaps(aabb2));
            prop_assert!(dist + 1e-9 >= radius_sum);
        }
    }
}
