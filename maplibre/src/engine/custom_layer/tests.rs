#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{CustomLayerContext, CustomLayerRegistry, CustomStyleLayer};
use crate::style::Style;

struct DummyCustomLayer {
    id: String,
}

impl CustomStyleLayer for DummyCustomLayer {
    fn id(&self) -> &str {
        &self.id
    }

    fn render<'pass>(
        &'pass self,
        _pass: &mut wgpu::RenderPass<'pass>,
        _ctx: &CustomLayerContext<'_>,
    ) {
    }
}

#[test]
fn style_parses_validates_and_mutates_custom_layer_without_source() {
    let mut style: Style = serde_json::from_value(serde_json::json!({
        "version": 8,
        "sources": {},
        "layers": [
            { "id": "bg", "type": "background", "paint": { "background-color": "#112233" } },
            { "id": "deck-overlay", "type": "custom", "minzoom": 2.0, "maxzoom": 18.0 }
        ]
    }))
    .expect("style with custom layer deserializes");

    assert_eq!(style.validate(), Ok(()));
    assert_eq!(style.layers[1].type_, "custom");
    assert!(style.layers[1].source.is_none());

    let change = style
        .add_layer(
            serde_json::json!({
                "id": "second-custom",
                "type": "custom"
            }),
            Some("deck-overlay"),
        )
        .expect("adding custom layer without source succeeds");
    assert!(!change.redraw_tiles);
    assert_eq!(style.layers[1].id, "second-custom");
    assert_eq!(style.layers[2].id, "deck-overlay");

    let serialized = serde_json::to_value(&style.layers[1]).expect("serializes");
    assert_eq!(serialized["type"], "custom");
}

#[test]
fn custom_layer_registry_insert_and_remove() {
    let mut registry = CustomLayerRegistry::new();
    assert!(!registry.contains("weather"));
    registry.register(DummyCustomLayer {
        id: "weather".into(),
    });
    assert!(registry.contains("weather"));
    assert!(registry.remove("weather"));
    assert!(!registry.contains("weather"));
    assert!(!registry.remove("weather"));
}

#[test]
fn uninitialized_layer_does_not_deinitialize_on_remove_replace_or_drop() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct CountingDummy {
        id: &'static str,
        deinit_count: Arc<AtomicUsize>,
    }

    impl CustomStyleLayer for CountingDummy {
        fn id(&self) -> &str {
            self.id
        }

        fn render<'pass>(
            &'pass self,
            _pass: &mut wgpu::RenderPass<'pass>,
            _ctx: &CustomLayerContext<'_>,
        ) {
        }

        fn deinitialize(&mut self) {
            self.deinit_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    let deinit = Arc::new(AtomicUsize::new(0));
    {
        let mut registry = CustomLayerRegistry::new();
        registry.register(CountingDummy {
            id: "a",
            deinit_count: Arc::clone(&deinit),
        });
        registry.register(CountingDummy {
            id: "a",
            deinit_count: Arc::clone(&deinit),
        });
        assert!(registry.remove("a"));
        registry.register(CountingDummy {
            id: "b",
            deinit_count: Arc::clone(&deinit),
        });
        registry.mark_all_uninitialized();
        registry.deinitialize_layers(&["b".to_owned()]);
    }
    assert_eq!(deinit.load(Ordering::SeqCst), 0);
}
