//! Reading a resource pack from a directory on disk.

use std::{collections::HashMap, path::Path};

use roadshield::ResourcePack;

use crate::PackLoadError;

/// Reads every file under `dir` and verifies them as a roadshield resource pack. Hosts that
/// keep the pack elsewhere, in an archive, an offline region or memory, build a
/// [`ResourcePack`] through their own [`roadshield::ResourceResolver`] instead.
pub fn load_pack_dir_blocking(dir: &Path) -> Result<ResourcePack, PackLoadError> {
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| PackLoadError::Io { path, source }
    };
    while let Some(current) = dirs.pop() {
        for entry in std::fs::read_dir(&current).map_err(io(&current))? {
            let path = entry.map_err(io(&current))?.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            // Pack paths are relative and use forward slashes on every platform.
            let Ok(relative) = path.strip_prefix(dir) else {
                continue;
            };
            let key = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.insert(key, std::fs::read(&path).map_err(io(&path))?);
        }
    }
    ResourcePack::load(&files).map_err(|source| PackLoadError::Pack {
        dir: dir.to_path_buf(),
        source,
    })
}
