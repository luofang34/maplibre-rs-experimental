//! Decoded glyph ranges and sprite sheets cached via [`AssetCache`].

use std::sync::Arc;

use prost::Message;
use rustc_hash::FxHashMap as HashMap;

use crate::{
    io::source_client::{AssetCache, AssetFailure, HttpClient, SourceClient},
    sdf::glyphs::Glyphs,
};

/// A sprite sheet decoded once for every icon later cut from it.
pub(super) struct SpriteSheet {
    pub(super) document: HashMap<String, super::sprite::SpriteEntry>,
    pub(super) image: Arc<image::RgbaImage>,
}

impl AssetCache {
    /// Decodes the glyph range compiled into the library, once.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) async fn bundled_glyphs(
        &self,
        bytes: &'static [u8],
    ) -> Result<Arc<Glyphs>, AssetFailure> {
        self.load("glyphs:bundled".to_owned(), || async {
            decode_glyphs(bytes, "bundled glyph range")
        })
        .await
    }
}

/// Fetches and decodes a glyph range, or returns the shared result of an earlier request.
pub(super) async fn load_glyphs<HC: HttpClient>(
    cache: &AssetCache,
    client: &SourceClient<HC>,
    url: &str,
) -> Result<Arc<Glyphs>, AssetFailure> {
    cache
        .load(format!("glyphs:{url}"), || async {
            let bytes = client.fetch_asset(url, "symbol glyph range").await?;
            decode_glyphs(&bytes, url)
        })
        .await
}

/// Fetches and decodes a sprite sheet, or returns the shared result of an earlier request.
pub(super) async fn load_sprites<HC: HttpClient>(
    cache: &AssetCache,
    client: &SourceClient<HC>,
    json_url: &str,
    png_url: &str,
) -> Result<Arc<SpriteSheet>, AssetFailure> {
    cache
        .load(format!("sprites:{json_url}|{png_url}"), || async {
            #[cfg(target_arch = "wasm32")]
            let (json, image) = crate::util::async_join::try_join2(
                client.fetch_asset(json_url, "symbol sprite metadata"),
                async {
                    if let Some(cached) =
                        crate::platform::http_client::cache::get_decoded_rgba_memory(png_url)
                    {
                        return Ok(cached);
                    }
                    let png = client.fetch_asset(png_url, "symbol sprite image").await?;
                    let image = Arc::new(crate::platform::decode_rgba_bytes(&png).await.map_err(
                        |error| {
                            tracing::warn!(%png_url, %error, "invalid sprite image");
                            AssetFailure::Terminal(format!(
                                "invalid sprite image {png_url}: {error}"
                            ))
                        },
                    )?);
                    crate::platform::http_client::cache::put_decoded_rgba_memory(png_url, &image);
                    Ok(image)
                },
            )
            .await?;
            #[cfg(not(target_arch = "wasm32"))]
            let (json, image) = {
                let (json, png) = crate::util::async_join::try_join2(
                    client.fetch_asset(json_url, "symbol sprite metadata"),
                    client.fetch_asset(png_url, "symbol sprite image"),
                )
                .await?;
                let image = Arc::new(
                    image::load_from_memory(&png)
                        .map_err(|error| {
                            tracing::warn!(%png_url, %error, "invalid sprite image");
                            AssetFailure::Terminal(format!(
                                "invalid sprite image {png_url}: {error}"
                            ))
                        })?
                        .to_rgba8(),
                );
                (json, image)
            };
            let document = super::sprite::parse_sprite_document(&json).map_err(|error| {
                tracing::warn!(%json_url, %error, "invalid sprite metadata");
                AssetFailure::Terminal(format!("invalid sprite metadata {json_url}: {error}"))
            })?;
            drop(json);
            let bytes = image.as_raw().len()
                + document.capacity()
                    * (std::mem::size_of::<String>()
                        + std::mem::size_of::<super::sprite::SpriteEntry>());
            Ok((SpriteSheet { document, image }, bytes))
        })
        .await
}

fn decode_glyphs(bytes: &[u8], source: &str) -> Result<(Glyphs, usize), AssetFailure> {
    let glyphs = Glyphs::decode(bytes).map_err(|error| {
        tracing::warn!(%source, %error, "invalid glyph range");
        AssetFailure::Terminal(format!("invalid glyph range {source}: {error}"))
    })?;
    let bytes = glyphs
        .stacks
        .iter()
        .flat_map(|stack| &stack.glyphs)
        .map(|glyph| std::mem::size_of_val(glyph) + glyph.bitmap.as_ref().map_or(0, Vec::len))
        .sum();
    Ok((glyphs, bytes))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
