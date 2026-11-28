//! Lays a tile's labels out again once the provided images they name have answers.

use std::sync::Arc;

use rustc_hash::FxHashSet as HashSet;

use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{run_cpu_bound_replying, Context, SendError},
        source_client::{HttpClient, SourceClient},
    },
    sdf::{
        assets::{load_symbol_assets_awaiting_from_tile, Resolved, SymbolAssetConfig},
        provided::{ProvidedImagesReport, ProvidedImagesState},
    },
    style::Style,
    vector::{
        process_vector::process_decoded_vector_tile_with_assets,
        transferables::VectorTransferables, ProcessVectorContext, ProcessVectorError,
        VectorTileRequest,
    },
};

/// What one source's symbols need from providers.
pub(super) struct Symbols {
    /// The source's decoded tile and symbol layers, kept while some images are still to come.
    pub(super) again: Option<(
        Arc<crate::vector::process_vector::DecodedVectorTile>,
        VectorTileRequest,
    )>,
    /// Device pixels per layout pixel the images are made for.
    pub(super) pixel_ratio: f32,
    /// Every provided name the labels ask for.
    pub(super) provided: Vec<String>,
    /// The names without an answer when the labels were laid out.
    pub(super) awaiting: Vec<String>,
}

/// The provided images of one tile's labels, across its sources.
pub(super) struct TileImages {
    coords: WorldTileCoords,
    attempt: Option<u64>,
    pixel_ratio: f32,
    /// Every provided name the labels ask for.
    provided: HashSet<String>,
    /// The sources whose labels lack some.
    waiting: Vec<Symbols>,
}

impl TileImages {
    pub(super) fn new(coords: WorldTileCoords, attempt: Option<u64>, pixel_ratio: f32) -> Self {
        Self {
            coords,
            attempt,
            pixel_ratio,
            provided: HashSet::default(),
            waiting: Vec::new(),
        }
    }

    /// Notes what one source's labels need.
    pub(super) fn add(&mut self, symbols: Option<Symbols>) {
        let Some(symbols) = symbols else {
            return;
        };
        self.provided.extend(symbols.provided.iter().cloned());
        if !symbols.awaiting.is_empty() {
            self.waiting.push(symbols);
        }
    }

    fn report(&self, state: ProvidedImagesState) -> ProvidedImagesReport {
        let mut names: Vec<String> = self.provided.iter().cloned().collect();
        names.sort_unstable();
        ProvidedImagesReport {
            coords: self.coords,
            attempt: self.attempt,
            names,
            pixel_ratio: self.pixel_ratio,
            state,
        }
    }

    /// Tells the map which provided images the labels sent so far name, and whether some are
    /// still to come.
    pub(super) fn report_first<C: Context>(&self, context: &C) -> Result<(), SendError> {
        if self.provided.is_empty() {
            return Ok(());
        }
        context.send_back(self.report(if self.waiting.is_empty() {
            ProvidedImagesState::Settled
        } else {
            ProvidedImagesState::Awaiting
        }))
    }

    /// Waits for the images the labels lack, lays out again the labels of each source that
    /// gained one, and reports whether any provider remains unavailable.
    pub(super) async fn lay_out_again<
        T: VectorTransferables,
        C: Context + Clone + Send,
        H: HttpClient,
    >(
        self,
        client: &SourceClient<H>,
        style: &Style,
        context: C,
    ) -> Result<(), SendError> {
        if self.waiting.is_empty() {
            return Ok(());
        }
        let coords = self.coords;
        let state = match lay_out_sources::<T, C, H>(
            client,
            style,
            coords,
            context.clone(),
            &self.waiting,
        )
        .await
        {
            Ok(true) => ProvidedImagesState::Retry,
            Ok(false) => ProvidedImagesState::Settled,
            Err(ProcessVectorError::SendError(source)) => return Err(source),
            Err(error) => {
                tracing::warn!(%coords, %error, "labels with provided images not laid out again");
                ProvidedImagesState::Retry
            }
        };
        context.send_back(self.report(state))
    }
}

/// Waits for the images `sources` lack and lays out again the labels of each source that
/// gained one. Returns whether any provider remains unavailable.
async fn lay_out_sources<T: VectorTransferables, C: Context + Clone + Send, H: HttpClient>(
    client: &SourceClient<H>,
    style: &Style,
    coords: WorldTileCoords,
    context: C,
    sources: &[Symbols],
) -> Result<bool, ProcessVectorError> {
    let (mut unavailable, gained) = resolve_missing_images(client, coords, sources).await;
    for source in sources {
        if source
            .awaiting
            .iter()
            .any(|name| gained.contains(name.as_str()))
        {
            unavailable |=
                relayout_source::<T, C, H>(client, style, coords, context.clone(), source).await?;
        }
    }
    Ok(unavailable)
}

async fn resolve_missing_images<H: HttpClient>(
    client: &SourceClient<H>,
    coords: WorldTileCoords,
    sources: &[Symbols],
) -> (bool, HashSet<String>) {
    let providers = client.image_providers();
    let mut wanted: Vec<(&str, f32)> = sources
        .iter()
        .flat_map(|source| {
            source
                .awaiting
                .iter()
                .map(|name| (name.as_str(), source.pixel_ratio))
        })
        .collect();
    wanted.sort_by(|a, b| a.0.cmp(b.0));
    wanted.dedup_by(|a, b| a.0 == b.0);
    // The names of a tile are asked for together; the registry runs each once however many
    // tiles wait for it, and bounds how many run at all.
    let answers = crate::util::async_join::join_all_concurrent(
        wanted
            .iter()
            .map(|(name, ratio)| providers.resolve(name, *ratio)),
    )
    .await;
    let mut unavailable = false;
    let mut gained = HashSet::default();
    for ((name, _), answer) in wanted.iter().zip(answers) {
        match answer.as_deref() {
            Ok(Resolved::Image(_)) => {
                gained.insert((*name).to_owned());
            }
            Ok(Resolved::None) => {}
            Err(reason) => {
                tracing::debug!(%coords, %name, %reason, "provided image unavailable for now");
                unavailable = true;
            }
        }
    }
    (unavailable, gained)
}

async fn relayout_source<T: VectorTransferables, C: Context + Clone + Send, H: HttpClient>(
    client: &SourceClient<H>,
    style: &Style,
    coords: WorldTileCoords,
    context: C,
    source: &Symbols,
) -> Result<bool, ProcessVectorError> {
    let Some((tile, request)) = &source.again else {
        return Ok(false);
    };
    let assets = load_symbol_assets_awaiting_from_tile(
        client,
        SymbolAssetConfig {
            pixel_ratio: source.pixel_ratio,
            ..SymbolAssetConfig::of(style)
        },
        &request.layers,
        tile,
        f64::from(request.overscaled_zoom.max(u8::from(coords.z))),
    )
    .await
    .map_err(ProcessVectorError::SymbolAssets)?;
    // An answer evicted from the cache falls back to the placeholder until the tile is retried.
    let still_awaiting = !assets.awaiting.is_empty();
    // The tile was completed already; these labels replace the ones it was drawn with.
    let source_state = request
        .layers
        .iter()
        .find_map(|l| l.source.as_deref())
        .map_or((None, None), |name| {
            (
                style.source_promote_id(name).cloned(),
                style.feature_state.source(name).cloned(),
            )
        });
    let (tile, request, atlas) = (tile.clone(), request.clone(), assets.atlas);
    run_cpu_bound_replying(context, move |context| {
        let mut processor = ProcessVectorContext::<T, _>::new(context)
            .without_completion()
            .without_indexing()
            .with_source_feature_state(source_state.0, source_state.1);
        process_decoded_vector_tile_with_assets(&tile, request, &mut processor, atlas)
    })
    .await
    .map_err(ProcessVectorError::CpuWork)??;
    client.image_providers().count_relaid();
    tracing::debug!(%coords, "labels laid out again with provided images");
    Ok(still_awaiting)
}
