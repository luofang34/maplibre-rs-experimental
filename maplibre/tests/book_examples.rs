//! The book's examples stay whole: every page the summary lists embeds a module whose code
//! tabs exist, and the interactive ones call only methods the web map offers.

#![allow(clippy::expect_used, clippy::panic)]

use std::{collections::BTreeSet, fs, path::Path};

fn read(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    fs::read_to_string(root.join(path)).expect(path)
}

/// The example pages the book's summary lists, by file stem.
fn listed_examples() -> Vec<String> {
    read("docs/src/SUMMARY-book.md")
        .lines()
        .filter_map(|line| line.split("(./examples/").nth(1))
        .filter_map(|rest| rest.split(".md)").next())
        .filter(|name| *name != "index")
        .map(str::to_owned)
        .collect()
}

/// The names of the methods `MapLibreMap` declares in `web/lib/src/map.ts`.
fn map_methods() -> BTreeSet<String> {
    let source = read("web/lib/src/map.ts");
    let class = source
        .split("export class MapLibreMap")
        .nth(1)
        .expect("the map class");
    class
        .lines()
        .filter(|line| line.starts_with("    ") && !line.starts_with("     "))
        .filter_map(|line| {
            let name: String = line
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let declares = line.trim_start()[name.len()..].starts_with('(');
            (declares && !name.is_empty()).then_some(name)
        })
        .collect()
}

/// The methods a module calls on the map it is given.
fn map_calls(module: &str) -> BTreeSet<String> {
    module
        .split("map.")
        .skip(1)
        .filter_map(|rest| {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            rest[name.len()..].starts_with('(').then_some(name)
        })
        .collect()
}

#[test]
fn every_listed_example_embeds_its_module_and_code_tabs() {
    let examples = listed_examples();
    assert!(examples.len() >= 4, "the summary lists examples");
    for name in examples {
        let page = read(&format!("docs/src/examples/{name}.md"));
        assert!(
            page.contains(&format!("?example=../code/{name}.js")),
            "{name} embeds its own module"
        );
        for tab in ["rs", "js", "html"] {
            let include = format!("{{{{#include code/{name}.{tab}}}}}");
            assert!(page.contains(&include), "{name} shows its {tab} tab");
            read(&format!("docs/src/examples/code/{name}.{tab}"));
        }
    }
}

#[test]
fn interactive_examples_call_only_methods_the_map_offers() {
    let offered = map_methods();
    for method in ["on", "flyTo", "queryRenderedFeatures", "setGeoJsonData"] {
        assert!(offered.contains(method), "{method} is read from map.ts");
    }
    let mut interactive = 0;
    for name in listed_examples() {
        let module = read(&format!("docs/src/examples/code/{name}.js"));
        if !module.contains("export function run(map)") {
            continue;
        }
        interactive += 1;
        let unknown: Vec<_> = map_calls(&module).difference(&offered).cloned().collect();
        assert!(
            unknown.is_empty(),
            "{name} calls {unknown:?}, which the map lacks"
        );
    }
    assert!(interactive >= 4, "the interactive examples are checked");
}

#[test]
fn the_demo_page_hands_the_map_to_an_examples_run() {
    let demo = read("web/demo/index.ts");
    assert!(
        demo.contains("module.run(map)"),
        "the embedded page calls the example's run export"
    );
}
