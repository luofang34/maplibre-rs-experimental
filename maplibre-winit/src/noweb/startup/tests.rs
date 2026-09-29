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

mod no_unwind;
