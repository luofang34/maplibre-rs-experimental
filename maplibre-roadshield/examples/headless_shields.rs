//! Draws OpenMapTiles-style roads with shields made on request from their route attributes,
//! and writes the frame as a PNG.
//!
//! `cargo run -p maplibre-roadshield --example headless_shields -- <pack dir> [out.png]`
//!
//! The pack is a roadshield resource pack, such as `packs/americana` of the roadshield
//! repository; `ROADSHIELD_PACK` names it when no argument does.

mod support;

use std::{error::Error, path::PathBuf};

use support::scene;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let pack = args
        .next()
        .or_else(|| std::env::var("ROADSHIELD_PACK").ok())
        .map(PathBuf::from)
        .ok_or("name a roadshield pack directory, or set ROADSHIELD_PACK")?;
    let out = args.next().unwrap_or_else(|| "shields.png".to_owned());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut map = scene::map(scene::provider(&pack)?, 2.0).await?;
        let pixels = scene::settle(&mut map).await?;
        let stats = map.image_providers().map(|providers| providers.stats());
        image::RgbaImage::from_raw(scene::SIZE, scene::SIZE, pixels)
            .ok_or("the frame has the wrong size")?
            .save(&out)?;
        tracing::info!(%out, ?stats, "shields drawn");
        Ok(())
    })
}
