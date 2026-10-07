//! The browser transport against real servers. Needs the network; run with
//! `CHROMEDRIVER=<path> cargo test -p maplibre --target wasm32-unknown-unknown --features
//! headless --test web_sources`.
#![cfg(target_arch = "wasm32")]

use maplibre::{
    io::source_client::{ByteRange, HttpClient},
    platform::http_client::{web_http_client, WHATWGFetchHttpClient},
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

/// A static archive served with CORS and range support.
const ARCHIVE: &str =
    "https://r2-public.protomaps.com/protomaps-sample-datasets/cb_2018_us_zcta510_500k.pmtiles";

#[wasm_bindgen_test]
async fn a_refused_cross_origin_request_names_its_url_and_is_retried() {
    // The server answers, but without CORS headers the page may not read the response.
    let url = "https://example.com/tiles/0/0/0.pbf";
    let error = WHATWGFetchHttpClient
        .fetch(url)
        .await
        .expect_err("the browser withholds the response");

    let description = error.describe();
    assert!(description.contains(url), "{description}");
    assert!(description.contains("CORS"), "{description}");
    assert!(
        error.is_retryable(),
        "a refusal looks like a network failure"
    );
    assert!(!error.is_not_found());
}

#[wasm_bindgen_test]
async fn a_range_request_reads_only_the_archive_header() {
    let header = WHATWGFetchHttpClient
        .fetch_range(
            ARCHIVE,
            ByteRange {
                offset: 0,
                length: 127,
            },
        )
        .await
        .expect("the header downloads");

    assert_eq!(header.len(), 127);
    assert_eq!(&header[..7], b"PMTiles");
    assert_eq!(header[7], 3, "spec version");
}

#[wasm_bindgen_test]
async fn a_pmtiles_archive_answers_with_tilejson_and_tiles() {
    let client = web_http_client();
    let archive = format!("pmtiles://{ARCHIVE}");
    let tile_json: serde_json::Value = serde_json::from_slice(
        &client
            .fetch(&archive)
            .await
            .expect("the archive's TileJSON"),
    )
    .expect("TileJSON parses");
    let template = tile_json["tiles"][0]
        .as_str()
        .expect("a tile template")
        .to_owned();
    let zoom = tile_json["minzoom"].as_u64().unwrap_or(0);

    // At zoom 0 the one tile covers the whole dataset.
    assert_eq!(zoom, 0, "the sample archive starts at zoom 0");
    let tile = client
        .fetch(
            &template
                .replace("{z}", "0")
                .replace("{x}", "0")
                .replace("{y}", "0"),
        )
        .await
        .expect("a tile reads out of the archive");
    assert!(!tile.is_empty());
}
