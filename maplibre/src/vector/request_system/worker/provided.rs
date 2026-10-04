//! Lays a tile's labels out again once the provided images they name have answers.

use std::collections::{BTreeSet, HashSet};

use crate::{
    coords::WorldTileCoords,
    io::{
        apc::{Context, SendError},
        source_client::{HttpClient, SourceClient},
    },
    sdf::{
        assets::{load_symbol_assets_awaiting, Resolved, SymbolAssetConfig},
        provided::{ProvidedImagesReport, ProvidedImagesState},
    },
    style::Style,
    vector::{
        process_vector::process_vector_tile_with_assets, transferables::VectorTransferables,
        ProcessVectorContext, ProcessVectorError, VectorTileRequest,
    },
};

/// What one source's symbols need from providers.
pub(super) struct Symbols {
    /// The source's tile and symbol layers, kept while some images are still to come.
    pub(super) again: Option<(Vec<u8>, VectorTileRequest)>,
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
    provided: BTreeSet<String>,
    /// The sources whose labels lack some.
    waiting: Vec<Symbols>,
}

impl TileImages {
    pub(super) fn new(coords: WorldTileCoords, attempt: Option<u64>, pixel_ratio: f32) -> Self {
        Self {
            coords,
            attempt,
            pixel_ratio,
            provided: BTreeSet::new(),
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
        ProvidedImagesReport {
            coords: self.coords,
            attempt: self.attempt,
            names: self.provided.iter().cloned().collect(),
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
    /// gained one, and reports whether some provider was unavailable for now.
    pub(super) async fn lay_out_again<T: VectorTransferables, C: Context + Clone, H: HttpClient>(
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
/// gained one. Returns whether some provider was unavailable for now.
async fn lay_out_sources<T: VectorTransferables, C: Context + Clone, H: HttpClient>(
    client: &SourceClient<H>,
    style: &Style,
    coords: WorldTileCoords,
    context: C,
    sources: &[Symbols],
) -> Result<bool, ProcessVectorError> {
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
    let answers = futures::future::join_all(
        wanted
            .iter()
            .map(|(name, pixel_ratio)| providers.resolve(name, *pixel_ratio)),
    )
    .await;
    let mut unavailable = false;
    let mut gained = HashSet::new();
    for ((name, _), answer) in wanted.iter().zip(answers) {
        match answer.as_deref() {
            Ok(Resolved::Image(_)) => {
                gained.insert(*name);
            }
            Ok(Resolved::None) => {}
            Err(reason) => {
                tracing::debug!(%coords, %name, %reason, "provided image unavailable for now");
                unavailable = true;
            }
        }
    }
    for source in sources {
        let Some((data, request)) = &source.again else {
            continue;
        };
        if !source
            .awaiting
            .iter()
            .any(|name| gained.contains(name.as_str()))
        {
            continue;
        }
        let assets = load_symbol_assets_awaiting(
            client,
            SymbolAssetConfig {
                pixel_ratio: source.pixel_ratio,
                ..SymbolAssetConfig::of(style)
            },
            &request.layers,
            data,
            f64::from(request.overscaled_zoom.max(u8::from(coords.z))),
        )
        .await
        .map_err(ProcessVectorError::SymbolAssets)?;
        // The tile was completed already; these labels replace the ones it was drawn with.
        let mut processor = ProcessVectorContext::<T, C>::new(context.clone()).without_completion();
        process_vector_tile_with_assets(data, request.clone(), &mut processor, assets.atlas)?;
        providers.count_relaid();
        tracing::debug!(%coords, images = gained.len(), "labels laid out again with provided images");
    }
    Ok(unavailable)
}
