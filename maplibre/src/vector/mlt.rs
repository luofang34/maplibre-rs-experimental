//! MapLibre Tile (MLT) payloads, read through the MVT decoder after a lossless conversion.

use geozero::mvt::Message;
use mlt_core::{mvt::tile_layers_to_mvt, Decoder, Layer, Parser};

/// The tile an MLT payload holds, in the form an MVT payload decodes to.
pub(super) fn decode(data: &[u8]) -> Result<geozero::mvt::Tile, Box<dyn std::error::Error>> {
    let layers = Parser::default().parse_layers(data)?;
    let mut decoder = Decoder::default();
    let mut tile_layers = Vec::with_capacity(layers.len());
    for layer in layers {
        if let Layer::Tag01(layer) = layer {
            tile_layers.push(layer.into_tile(&mut decoder)?);
        }
    }
    let mvt = tile_layers_to_mvt(tile_layers)?;
    Ok(geozero::mvt::Tile::decode(mvt.as_slice())?)
}

#[cfg(test)]
mod tests;
