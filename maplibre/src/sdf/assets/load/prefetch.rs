//! Concurrent startup prefetch for `HTTPS` sprite sheets and `0-255` Latin glyph ranges.

use super::sprite_sources;
#[cfg(any(
    target_arch = "wasm32",
    all(feature = "native-io", feature = "thread-safe-futures")
))]
use super::{fetch_sprite_sheet, glyph_range};
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
use super::{glyph_url, sprite_url};
use crate::{
    io::source_client::{HttpClient, SourceClient},
    style::{layer::LayerPaint, Style},
};

pub(crate) struct PrefetchPlan {
    has_symbols: bool,
    sources: Vec<(String, String)>,
    stacks: Vec<String>,
    #[cfg_attr(
        not(any(
            target_arch = "wasm32",
            all(feature = "native-io", feature = "thread-safe-futures")
        )),
        allow(dead_code)
    )]
    glyph_template: Option<String>,
}

impl PrefetchPlan {
    pub(crate) fn from_style(style: &Style) -> Self {
        let mut stacks = Vec::new();
        let mut has_symbols = false;
        for layer in &style.layers {
            if layer.type_ == "symbol" && !layer.is_hidden() {
                has_symbols = true;
                if let Some(LayerPaint::Symbol(paint)) = &layer.paint {
                    if paint.text_field.is_some() && paint.properties.contains_key("text-font") {
                        let stack = paint.font_stack();
                        if !stack.is_empty() && !stacks.contains(&stack) && stacks.len() < 4 {
                            stacks.push(stack);
                        }
                    }
                }
            }
        }
        let sources: Vec<_> = if has_symbols {
            sprite_sources(style.sprite.as_ref())
                .into_iter()
                .filter(|(_, url)| url.starts_with("https://"))
                .collect()
        } else {
            Vec::new()
        };
        let glyph_template = has_symbols
            .then_some(style.glyphs.as_deref())
            .flatten()
            .filter(|url| url.starts_with("https://"))
            .map(str::to_owned);
        Self {
            has_symbols,
            sources,
            stacks,
            glyph_template,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        !self.has_symbols || (self.sources.is_empty() && self.stacks.is_empty())
    }

    #[cfg(all(
        not(target_arch = "wasm32"),
        feature = "native-io",
        feature = "thread-safe-futures"
    ))]
    async fn execute_decoded<HC: HttpClient>(self, client: &SourceClient<HC>) {
        if self.is_empty() {
            return;
        }
        let tpl = self.glyph_template.as_deref();
        let sprite_fut = futures::future::join_all(
            self.sources
                .iter()
                .map(|(prefix, url)| fetch_sprite_sheet(client, prefix, url)),
        );
        let glyph_fut = futures::future::join_all(tpl.into_iter().flat_map(|t| {
            self.stacks
                .iter()
                .map(move |stack| glyph_range(client, Some(t), stack, 0))
        }));
        let _ = futures::future::join(sprite_fut, glyph_fut).await;
    }

    /// Warms raw HTTP/CacheStorage entries once on the map worker without PNG/PBF CPU decoding.
    #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
    async fn execute_warm_raw<HC: HttpClient>(self, client: &SourceClient<HC>) {
        if self.is_empty() {
            return;
        }
        let urls: Vec<String> = self
            .sources
            .iter()
            .flat_map(|(_, url)| [sprite_url(url, "json"), sprite_url(url, "png")])
            .chain(self.glyph_template.as_deref().into_iter().flat_map(|tpl| {
                self.stacks
                    .iter()
                    .map(move |stack| glyph_url(tpl, stack, 0))
            }))
            .collect();
        let _ = crate::util::async_join::join_all_concurrent(
            urls.iter().map(|url| client.fetch_url(url)),
        )
        .await;
    }

    /// Pre-fetches and decodes sprites and `0-255` glyph ranges into `client`'s `AssetCache`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn execute_decoded_wasm<HC: HttpClient>(self, client: &SourceClient<HC>) {
        if self.is_empty() {
            return;
        }
        let tpl = self.glyph_template.as_deref();
        let sprite_fut = async {
            let _ = crate::util::async_join::join_all_concurrent(
                self.sources
                    .iter()
                    .map(|(prefix, url)| fetch_sprite_sheet(client, prefix, url)),
            )
            .await;
            Ok::<(), ()>(())
        };
        let glyph_fut = async {
            if let Some(tpl) = tpl {
                let _ = crate::util::async_join::join_all_concurrent(
                    self.stacks
                        .iter()
                        .map(|stack| glyph_range(client, Some(tpl), stack, 0)),
                )
                .await;
            }
            Ok::<(), ()>(())
        };
        if crate::util::async_join::try_join2(sprite_fut, glyph_fut)
            .await
            .is_err()
        {}
    }
}

/// Pre-fetches and decodes `HTTPS` sprites and `0-255` glyph ranges into a tile worker's cache.
pub fn spawn_worker_symbol_prefetch<HC: HttpClient + 'static>(
    client: SourceClient<HC>,
    style: &Style,
) {
    let plan = PrefetchPlan::from_style(style);
    if plan.is_empty() {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        plan.execute_decoded_wasm(&client).await;
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (client, plan);
}

/// Fires `HTTPS` sprite and `0-255` glyph prefetch in the background without blocking map
/// initialization on remote asset latency.
#[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
pub(crate) fn spawn_style_symbol_prefetch<HC: HttpClient>(client: SourceClient<HC>, style: &Style) {
    let plan = PrefetchPlan::from_style(style);
    if plan.is_empty() {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        plan.execute_warm_raw(&client).await;
    });
    #[cfg(all(
        not(target_arch = "wasm32"),
        feature = "native-io",
        feature = "thread-safe-futures"
    ))]
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            plan.execute_decoded(&client).await;
        });
    }
    #[cfg(all(
        not(target_arch = "wasm32"),
        not(all(feature = "native-io", feature = "thread-safe-futures"))
    ))]
    let _ = (client, plan);
}
