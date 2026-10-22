#![allow(clippy::expect_used, clippy::panic)]

use super::*;

#[test]
fn cache_path_remains_exact_and_optional() {
    assert_eq!(cache_directory(None).expect("disabled cache"), None);
    let path = PathBuf::from("cache/地图");
    assert_eq!(
        cache_directory(Some(&path)).expect("UTF-8 path"),
        Some("cache/地图".into())
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_cache_path_is_rejected_before_starting_a_window() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let path = PathBuf::from(OsString::from_vec(b"cache/\xff".to_vec()));
    let error = run_headed_map(
        Some(path.clone()),
        WinitMapWindowConfig::new("invalid cache".into()),
        WgpuSettings::default(),
        Style::default(),
        HeadedMapOptions::default(),
    )
    .expect_err("path must be reported without unwinding or creating a window");
    let HeadedMapError::InvalidCachePath { path: rejected } = error else {
        panic!("wrong cause")
    };
    assert_eq!(rejected, path);
}

/// A plugin that registers nothing, standing for a host's own.
struct Inert;

impl Plugin<NativeEnvironment> for Inert {
    fn build(
        &self,
        _: &mut maplibre::schedule::Schedule,
        _: std::rc::Rc<Kernel<NativeEnvironment>>,
        _: &mut maplibre::tcs::world::World,
        _: &mut maplibre::render::graph::RenderGraph,
    ) {
    }
}

#[test]
fn a_host_adds_its_plugins_after_the_ones_the_style_needs() {
    let style = Style::default();
    let needed = default_plugins(&style, false).len();
    let mut host =
        HeadedMapHost::default().with_plugins(|_| vec![Box::new(Inert), Box::new(Inert)]);
    assert_eq!(host.plugins_for(&style, false).len(), needed + 2);
    assert_eq!(
        host.plugins_for(&style, true).len(),
        needed + 3,
        "the tile outlines join the style's plugins and the host's are built again"
    );
    assert_eq!(
        HeadedMapHost::default().plugins_for(&style, false).len(),
        needed
    );
}

mod no_unwind;
