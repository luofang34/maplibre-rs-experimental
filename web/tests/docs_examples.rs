//! The browser-side Rust examples of the book compile against the web crate and build valid
//! styles; the native ones are checked by `maplibre-demo/tests/docs_examples.rs`.
#![allow(clippy::expect_used)]

use maplibre::style::Style;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[allow(dead_code)]
mod fly_to_a_location {
    include!("../../docs/src/examples/code/fly-to-a-location.rs");
}

#[allow(dead_code)]
mod get_features_under_the_mouse_pointer {
    include!("../../docs/src/examples/code/get-features-under-the-mouse-pointer.rs");
}

#[allow(dead_code)]
mod change_a_layers_color {
    include!("../../docs/src/examples/code/change-a-layers-color.rs");
}

#[allow(dead_code)]
mod show_and_hide_layers {
    include!("../../docs/src/examples/code/show-and-hide-layers.rs");
}

#[allow(dead_code)]
mod filter_symbols_by_toggling_a_list {
    include!("../../docs/src/examples/code/filter-symbols-by-toggling-a-list.rs");
}

#[allow(dead_code)]
mod add_live_realtime_data {
    include!("../../docs/src/examples/code/add-live-realtime-data.rs");
}

#[wasm_bindgen_test]
fn browser_examples_build_valid_styles() {
    let styles = [
        (
            "fly-to-a-location",
            fly_to_a_location::build_style(Style::default()),
        ),
        (
            "get-features-under-the-mouse-pointer",
            get_features_under_the_mouse_pointer::build_style(Style::default()),
        ),
        (
            "change-a-layers-color",
            change_a_layers_color::build_style(Style::default()).expect("style document"),
        ),
        (
            "show-and-hide-layers",
            show_and_hide_layers::build_style(Style::default()),
        ),
        (
            "filter-symbols-by-toggling-a-list",
            filter_symbols_by_toggling_a_list::build_style(Style::default())
                .expect("style document"),
        ),
        (
            "add-live-realtime-data",
            add_live_realtime_data::build_style(Style::default()).expect("style document"),
        ),
    ];
    for (name, style) in styles {
        let issues = style.validate();
        assert!(
            issues.is_ok(),
            "example `{name}` produced style validation issues: {issues:?}"
        );
    }
}
