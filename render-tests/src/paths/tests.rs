use super::collect_tests;

#[test]
fn fixtures_are_found_however_deep_they_are_nested() {
    let root = tempfile::tempdir().expect("temporary directory");
    let shallow = root.path().join("circle-radius/literal");
    let deep = root
        .path()
        .join("projection/globe/terrain/circle-pitch-alignment/map-scale-map");
    for fixture in [&shallow, &deep] {
        std::fs::create_dir_all(fixture).expect("fixture directory");
        std::fs::write(fixture.join("style.json"), "{}").expect("style");
    }
    assert_eq!(collect_tests(root.path()), [shallow, deep]);
}
