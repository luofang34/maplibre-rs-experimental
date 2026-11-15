//! Concurrent startup prefetch for `HTTPS` sprite sheets and `0-255` Latin glyph ranges.

use super::{fetch_sprite_sheet, glyph_range, sprite_sources};
use crate::{
    io::source_client::{HttpClient, SourceClient},
    style::{layer::LayerPaint, Style},
};

#[cfg_attr(
    not(any(
        target_arch = "wasm32",
        all(feature = "native-io", feature = "thread-safe-futures")
    )),
    allow(dead_code)
)]
struct PrefetchPlan {
    has_symbols: bool,
    sources: Vec<(String, String)>,
    stacks: Vec<String>,
    glyph_template: Option<String>,
}

#[cfg_attr(
    not(any(
        target_arch = "wasm32",
        all(feature = "native-io", feature = "thread-safe-futures")
    )),
    allow(dead_code)
)]
impl PrefetchPlan {
    fn from_style(style: &Style) -> Self {
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

    async fn execute_decoded<HC: HttpClient>(self, client: &SourceClient<HC>) {
        if !self.has_symbols || (self.sources.is_empty() && self.stacks.is_empty()) {
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
}

/// Awaits `HTTPS` sprite sheets and `0-255` Latin glyph ranges into `client`'s `AssetCache`
/// before heavy tile MVT AST expansion so decoded tiles never sit in memory across asset I/O.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn warm_style_symbol_assets<HC: HttpClient>(
    client: &SourceClient<HC>,
    style: &Style,
) {
    let plan = PrefetchPlan::from_style(style);
    plan.execute_decoded(client).await;
}

/// Fires `HTTPS` sprite and `0-255` glyph prefetch in the background without blocking map
/// initialization on remote asset latency.
pub(crate) fn spawn_style_symbol_prefetch<HC: HttpClient>(client: SourceClient<HC>, style: &Style) {
    // On WASM, offscreen workers run `warm_style_symbol_assets` / `spawn_worker_symbol_prefetch`
    // off the main thread concurrently with tile fetches, avoiding main-thread allocator traffic.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (client, style);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let plan = PrefetchPlan::from_style(style);
        if !plan.has_symbols || (plan.sources.is_empty() && plan.stacks.is_empty()) {
            return;
        }
        #[cfg(all(feature = "native-io", feature = "thread-safe-futures"))]
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                plan.execute_decoded(&client).await;
            });
        }
        #[cfg(not(all(feature = "native-io", feature = "thread-safe-futures")))]
        let _ = (client, plan);
    }
}

/// Decodes `HTTPS` sprites and `0-255` Latin glyph ranges into an offscreen worker's
/// `AssetCache` concurrently with incoming tile fetches.
#[doc(hidden)]
pub fn spawn_worker_symbol_prefetch<HC: HttpClient>(client: SourceClient<HC>, style: &Style) {
    let plan = PrefetchPlan::from_style(style);
    if !plan.has_symbols || (plan.sources.is_empty() && plan.stacks.is_empty()) {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        plan.execute_decoded(&client).await;
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (client, plan);
}
