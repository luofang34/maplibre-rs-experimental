#[cfg(unix)]
#[test]
fn invalid_cache_path_returns_without_unwinding() {
    use crate::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};
    use maplibre::{render::settings::WgpuSettings, style::Style};
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};

    let result = std::panic::catch_unwind(|| {
        drop(run_headed_map(
            Some(PathBuf::from(OsString::from_vec(b"cache/\xff".to_vec()))),
            WinitMapWindowConfig::new("invalid cache".into()),
            WgpuSettings::default(),
            Style::default(),
            HeadedMapOptions::default(),
        ));
    });
    assert!(result.is_ok(), "invalid cache paths must not unwind");
}
