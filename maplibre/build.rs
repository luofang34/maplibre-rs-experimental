//! Validates shaders, generates protobuf bindings, and optionally embeds vector tiles.
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use maplibre_build_tools::wgsl::validate_project_wgsl_blocking;

type BuildResult = Result<(), Box<dyn std::error::Error>>;

#[cfg(feature = "embed-static-tiles")]
fn embed_tiles_statically_blocking() -> BuildResult {
    use maplibre_build_tools::mbtiles::extract_blocking;
    use std::{env, path::Path};

    const MUNICH_X: u32 = 17425;
    const MUNICH_Y: u32 = 11365;
    const MUNICH_Z: u8 = 15;
    let out = Path::new(&env::var("OUT_DIR")?).join("extracted-tiles");
    if out.is_dir() {
        fs::remove_dir_all(&out)?;
    }
    let source = Path::new(&env::var("CARGO_MANIFEST_DIR")?)
        .join(format!("../test-data/munich-{MUNICH_Z}.mbtiles"));
    if source.exists() {
        writeln!(io::stdout().lock(), "cargo:rustc-cfg=static_tiles_found")?;
        extract_blocking(
            source,
            out,
            MUNICH_Z,
            (MUNICH_X - 2)..(MUNICH_X + 2),
            (MUNICH_Y - 2)..(MUNICH_Y + 2),
        )?;
    }
    Ok(())
}

fn generate_protobuf_blocking() -> BuildResult {
    let mut paths = Vec::new();
    for entry in fs::read_dir("./proto")? {
        let path = entry?.path();
        writeln!(
            io::stdout().lock(),
            "cargo:rerun-if-changed={}",
            path.display()
        )?;
        paths.push(path);
    }
    prost_build::compile_protos(&paths, &[PathBuf::from("./proto/")])?;
    Ok(())
}

fn main() -> BuildResult {
    writeln!(
        io::stdout().lock(),
        "cargo:rustc-check-cfg=cfg(static_tiles_found)"
    )?;
    validate_project_wgsl_blocking()?;
    #[cfg(feature = "embed-static-tiles")]
    embed_tiles_statically_blocking()?;
    generate_protobuf_blocking()
}
