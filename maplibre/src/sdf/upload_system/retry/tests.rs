#![allow(clippy::expect_used, clippy::panic)]
use super::*;

#[tokio::test]
async fn failed_symbol_upload_keeps_atlas_and_features_while_refreshing_paint() {
    let mut context = context().await;
    let old = atlas(80);
    content::accept_symbols(&mut context.world, layer(3, old.clone(), "old"));
    upload(&mut context, 0.0);
    let old_allocation = allocation(&context);
    let old_group = binding(&context).group.clone();
    assert!(!binding(&context).separate_halo);
    let new = atlas(200);
    content::accept_symbols(&mut context.world, layer(257, new.clone(), "new"));
    upload(&mut context, 4.0);
    assert_eq!(
        allocation(&context),
        old_allocation,
        "capacity rejection preserves geometry"
    );
    assert_eq!(
        binding(&context).group,
        old_group,
        "atlas binding stays committed"
    );
    assert!(
        binding(&context).separate_halo,
        "retained symbols still evaluate current zoom paint"
    );
    assert_eq!(committed(&context).features[0].str, "old");
    assert!(Arc::ptr_eq(
        committed(&context).atlas.as_ref().expect("atlas"),
        &old
    ));
    content::accept_symbols(&mut context.world, layer(3, new.clone(), "new"));
    upload(&mut context, 4.0);
    assert_ne!(allocation(&context), old_allocation);
    assert_ne!(
        binding(&context).group,
        old_group,
        "new atlas changes with the geometry"
    );
    assert_eq!(committed(&context).features[0].str, "new");
    assert!(Arc::ptr_eq(
        committed(&context).atlas.as_ref().expect("atlas"),
        &new
    ));
    assert!(context
        .world
        .tiles
        .query::<&content::LayerReplacements>(Default::default())
        .expect("pending")
        .symbols
        .is_empty());
}
