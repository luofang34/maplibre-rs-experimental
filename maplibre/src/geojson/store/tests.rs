#![allow(clippy::expect_used, clippy::panic)]
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::{
    io::source_client::{HttpClient, HttpSourceClient, SourceClient, SourceFetchError},
    sdf::assets::{AssetCache, AssetFailure},
    style::source::GeoJsonSource,
};

#[derive(Clone, Default)]
struct Documents {
    calls: Arc<Mutex<Vec<String>>>,
    mode: Arc<Mutex<Mode>>,
}

#[derive(Clone, Copy, Default)]
enum Mode {
    #[default]
    Ok,
    Missing,
    Broken,
    Transient,
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait::async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait::async_trait)]
impl HttpClient for Documents {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, SourceFetchError> {
        self.calls.lock().expect("calls").push(url.to_owned());
        tokio::task::yield_now().await;
        match *self.mode.lock().expect("mode") {
            Mode::Ok => Ok(br#"{"type":"Point","coordinates":[1,2]}"#.to_vec()),
            Mode::Missing => Err(SourceFetchError::not_found(url)),
            Mode::Broken => Ok(b"<html>".to_vec()),
            Mode::Transient => Err(SourceFetchError::temporary(std::io::Error::other("reset"))),
        }
    }
}

fn url_source(generation: u64) -> GeoJsonSource {
    let mut source: GeoJsonSource =
        serde_json::from_value(json!({"data": "https://data.invalid/a.geojson"})).expect("source");
    source.generation = generation;
    source
}

fn client(documents: &Documents) -> SourceClient<Documents> {
    SourceClient::new(HttpSourceClient::new(documents.clone()))
}

#[tokio::test]
async fn concurrent_tiles_fetch_and_parse_the_document_once() {
    let documents = Documents::default();
    let (client, cache) = (client(&documents), AssetCache::default());
    let source = url_source(0);
    let (a, b, c) = tokio::join!(
        cache.geojson_index(&client, "places", &source),
        cache.geojson_index(&client, "places", &source),
        cache.geojson_index(&client, "places", &source),
    );
    let (a, b, c) = (a.expect("a"), b.expect("b"), c.expect("c"));
    assert!(Arc::ptr_eq(&a, &b) && Arc::ptr_eq(&b, &c));
    assert_eq!(documents.calls.lock().expect("calls").len(), 1);
}

#[tokio::test]
async fn a_new_generation_loads_the_data_again_and_sources_do_not_share() {
    let documents = Documents::default();
    let (client, cache) = (client(&documents), AssetCache::default());
    cache
        .geojson_index(&client, "places", &url_source(0))
        .await
        .expect("first");
    cache
        .geojson_index(&client, "places", &url_source(0))
        .await
        .expect("cached");
    cache
        .geojson_index(&client, "places", &url_source(1))
        .await
        .expect("next generation");
    cache
        .geojson_index(&client, "other", &url_source(0))
        .await
        .expect("other source");
    assert_eq!(documents.calls.lock().expect("calls").len(), 3);
}

#[tokio::test]
async fn inline_data_needs_no_request() {
    let documents = Documents::default();
    let (client, cache) = (client(&documents), AssetCache::default());
    let source: GeoJsonSource =
        serde_json::from_value(json!({"data": {"type": "Point", "coordinates": [0, 0]}}))
            .expect("source");
    let index = cache
        .geojson_index(&client, "here", &source)
        .await
        .expect("index");
    assert_eq!(index.len(), 1);
    assert!(documents.calls.lock().expect("calls").is_empty());
}

#[tokio::test]
async fn missing_and_broken_documents_are_typed_and_remembered_but_transient_ones_are_not() {
    let documents = Documents::default();
    let (client, cache) = (client(&documents), AssetCache::default());
    *documents.mode.lock().expect("mode") = Mode::Missing;
    for _ in 0..2 {
        assert_eq!(
            cache
                .geojson_index(&client, "gone", &url_source(0))
                .await
                .err(),
            Some(AssetFailure::NotFound)
        );
    }
    *documents.mode.lock().expect("mode") = Mode::Broken;
    for _ in 0..2 {
        assert!(matches!(
            cache
                .geojson_index(&client, "broken", &url_source(0))
                .await
                .err(),
            Some(AssetFailure::Terminal(_))
        ));
    }
    *documents.mode.lock().expect("mode") = Mode::Transient;
    for _ in 0..2 {
        assert!(matches!(
            cache
                .geojson_index(&client, "flaky", &url_source(0))
                .await
                .err(),
            Some(AssetFailure::Retryable(_))
        ));
    }
    assert_eq!(
        documents.calls.lock().expect("calls").len(),
        4,
        "1 + 1 + 2 requests"
    );
    *documents.mode.lock().expect("mode") = Mode::Ok;
    cache
        .geojson_index(&client, "flaky", &url_source(0))
        .await
        .expect("recovers");
}

#[tokio::test]
async fn two_styles_with_one_source_name_never_share_an_index() {
    let documents = Documents::default();
    let (client, cache) = (client(&documents), AssetCache::default());
    let declare = |x: f64| -> GeoJsonSource {
        serde_json::from_value(json!({"data": {"type": "Point", "coordinates": [x, 0.0]}}))
            .expect("source")
    };
    let (first, second) = (declare(1.0), declare(2.0));
    assert_ne!(
        first.generation, second.generation,
        "each parse gets its own generation"
    );
    let a = cache
        .geojson_index(&client, "points", &first)
        .await
        .expect("first");
    let b = cache
        .geojson_index(&client, "points", &second)
        .await
        .expect("second");
    assert!(!Arc::ptr_eq(&a, &b));
    let again = cache
        .geojson_index(&client, "points", &first.clone())
        .await
        .expect("clone");
    assert!(
        Arc::ptr_eq(&a, &again),
        "a clone of one style shares its data"
    );
}

#[tokio::test]
async fn a_source_sent_to_a_worker_as_a_message_keeps_its_generation_and_shares_one_index() {
    let documents = Documents::default();
    let (client, cache) = (client(&documents), AssetCache::default());
    let declared: GeoJsonSource =
        serde_json::from_value(json!({"data": "https://data.invalid/a.geojson"})).expect("source");
    let message = serde_json::to_string(&declared).expect("serializes");
    let first: GeoJsonSource = serde_json::from_str(&message).expect("first request");
    let second: GeoJsonSource = serde_json::from_str(&message).expect("second request");
    assert_eq!(first.generation, declared.generation);
    assert_eq!(second.generation, declared.generation);
    let a = cache
        .geojson_index(&client, "places", &first)
        .await
        .expect("first");
    let b = cache
        .geojson_index(&client, "places", &second)
        .await
        .expect("second");
    assert!(Arc::ptr_eq(&a, &b), "both tile requests use one index");
    assert_eq!(
        documents.calls.lock().expect("calls").len(),
        1,
        "the document is fetched once"
    );
}
