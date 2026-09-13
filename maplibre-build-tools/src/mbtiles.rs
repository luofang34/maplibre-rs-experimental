//! Extract gzipped vector tiles and metadata from an MBTiles database.
use std::{collections::HashMap, fs, fs::File, io, io::Write, ops::Range, path::Path};

use flate2::bufread::GzDecoder;
use rusqlite::{params, Connection, Row};

/// An extraction failure qualified by the input database and output directory.
#[derive(Debug, thiserror::Error)]
#[error("extracting {} into {}: {source}", input.display(), output.display())]
pub struct Error {
    input: std::path::PathBuf,
    output: std::path::PathBuf,
    #[source]
    source: ExtractionError,
}

/// Extracts gzipped vector tiles and metadata into a new output directory.
pub fn extract_blocking(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    z: u8,
    x_range: Range<u32>,
    y_range: Range<u32>,
) -> Result<(), Error> {
    extract_tiles_blocking(input.as_ref(), output.as_ref(), z, x_range, y_range).map_err(|source| {
        Error {
            input: input.as_ref().into(),
            output: output.as_ref().into(),
            source,
        }
    })
}

/// An extraction failure with the original operation error.
#[derive(Debug, thiserror::Error)]
enum ExtractionError {
    /// Invalid input or output path.
    #[error("{0}")]
    Path(String),
    /// Filesystem or decompression failure.
    #[error("extracting MBTiles data: {0}")]
    Io(#[from] io::Error),
    /// SQLite query or row decoding failure.
    #[error("reading MBTiles database: {0}")]
    Sql(#[from] rusqlite::Error),
    /// Metadata encoding failure.
    #[error("encoding MBTiles metadata: {0}")]
    Json(#[from] serde_json::Error),
}

/// Extracts a rectangular tile selection and the database metadata.
///
/// The output directory must not exist. Input tile data must be gzip compressed.
fn extract_tiles_blocking<P: AsRef<Path>, R: AsRef<Path>>(
    input_mbtiles: P,
    output_dir: R,
    z: u8,
    x_range: Range<u32>,
    y_range: Range<u32>,
) -> Result<(), ExtractionError> {
    let input_path = input_mbtiles.as_ref().to_path_buf();
    if !input_path.is_file() {
        return Err(ExtractionError::Path(format!(
            "Input file {input_path:?} is not a file",
        )));
    }

    let output_path = output_dir.as_ref().to_path_buf();
    if output_path.exists() {
        return Err(ExtractionError::Path(format!(
            "Output directory {output_path:?} already exists"
        )));
    }
    let connection = Connection::open(input_path)?;

    fs::create_dir_all(&output_path)?;

    extract_metadata(&connection, &output_path)?;

    // language=SQL
    let mut prepared_statement = connection.prepare(
        "SELECT zoom_level, tile_column, tile_row, tile_data
                        FROM tiles
                        WHERE   (zoom_level = ?1) AND
                                (tile_column BETWEEN ?2 AND ?3) AND
                                (tile_row BETWEEN ?4 AND ?5);",
    )?;

    let mut tiles_rows = prepared_statement.query(params![
        z,
        x_range.start,
        x_range.end,
        flip_vertical_axis(z, y_range.end),
        flip_vertical_axis(z, y_range.start) // in mbtiles it is stored flipped
    ])?;

    while let Some(tile) = tiles_rows.next()? {
        extract_tile(tile, &output_path)?;
    }

    Ok(())
}

fn flip_vertical_axis(zoom: u8, value: u32) -> u32 {
    2u32.pow(zoom as u32) - 1 - value
}

fn extract_tile(tile: &Row, output_path: &Path) -> Result<(), ExtractionError> {
    let (z, x, mut y): (u8, u32, u32) = (
        tile.get::<_, u8>(0)?,
        tile.get::<_, u32>(1)?,
        tile.get::<_, u32>(2)?,
    );

    // Flip vertical axis
    y = flip_vertical_axis(z, y);

    let tile_dir = output_path.join(format!("{z}/{x}"));

    fs::create_dir_all(&tile_dir)?;

    let tile_path = tile_dir.join(format!("{y}.pbf"));
    let tile_data = tile.get::<_, Vec<u8>>(3)?;
    let mut decoder = GzDecoder::new(tile_data.as_ref());

    let mut tile_file = File::create(tile_path)?;
    io::copy(&mut decoder, &mut tile_file)?;
    Ok(())
}

fn extract_metadata(connection: &Connection, output_path: &Path) -> Result<(), ExtractionError> {
    // language=SQL
    let mut prepared_statement = connection.prepare("SELECT name, value FROM metadata;")?;
    let metadata = prepared_statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let metadata_map = metadata.collect::<Result<HashMap<String, String>, _>>()?;

    let json_string = serde_json::to_string(&metadata_map)?;
    let metadata_path = output_path.join("metadata.json");
    let mut metadata_file = File::create(metadata_path)?;
    metadata_file.write_all(json_string.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests;
