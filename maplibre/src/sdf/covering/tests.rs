use super::*;

#[test]
fn parent_labels_remain_until_every_visible_replacement_is_ready() {
    let parent = WorldTileCoords::default();
    let children = parent.get_children();
    for count in 0..children.len() {
        let ready: HashSet<_> = std::iter::once(parent)
            .chain(children.iter().take(count).copied())
            .collect();
        let result = select(&children, |tile| ready.contains(&tile));
        assert!(result.contains(&parent));
        assert_eq!(result.len(), count + 1);
        assert!(children
            .iter()
            .take(count)
            .all(|child| result.contains(child)));
    }
    let mut expected = children.to_vec();
    expected.sort_by_key(|tile| (tile.z, tile.y, tile.x));
    assert_eq!(select(&children, |_| true), expected);
    for _ in 0..100 {
        assert_eq!(select(&children, |_| true), expected);
    }
}

#[test]
fn missing_assets_do_not_clear_an_available_ancestor() {
    let parent = WorldTileCoords::default();
    let child = parent.get_children()[0];
    let grandchild = child.get_children()[0];
    assert_eq!(select(&[grandchild], |tile| tile == parent), vec![parent]);
    assert_eq!(
        select(&[grandchild], |tile| tile == child || tile == parent),
        vec![child]
    );
    assert_eq!(
        select(&[grandchild], |tile| tile == grandchild),
        vec![grandchild]
    );
}
