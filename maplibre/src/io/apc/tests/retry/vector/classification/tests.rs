use super::super::super::{
    fixture::{Fixture, Kind},
    source::Response,
};

#[tokio::test]
async fn vector_transport_timeout_rate_limit_and_server_failures_retry() {
    for response in [
        Response::Disconnect,
        Response::Status(408),
        Response::Status(429),
        Response::Status(500),
        Response::Status(502),
    ] {
        let mut test = Fixture::new(Kind::Vector, false).await;
        test.source.set(response);
        test.frame(0);
        test.receive().await;
        assert!(!test.loaded());
        test.source.set(Response::Bytes(super::super::tile()));
        test.frame(1000);
        test.receive().await;
        assert!(
            test.loaded(),
            "temporary failure recovers in the same component"
        );
    }
}

#[tokio::test]
async fn vector_missing_client_and_decode_failures_remain_terminal() {
    for response in [
        Response::Status(404),
        Response::Status(403),
        Response::Status(400),
        Response::Corrupt,
    ] {
        let mut test = Fixture::new(Kind::Vector, false).await;
        test.source.set(response);
        test.frame(0);
        test.receive().await;
        assert!(!test.loaded());
        test.source.set(Response::Bytes(super::super::tile()));
        for time in [1000, 30000, 60000] {
            test.frame(time);
            test.receive().await;
        }
        assert_eq!(
            test.source.requests(),
            1,
            "terminal failures do not retry every frame"
        );
    }
}

#[tokio::test]
async fn healthy_vector_source_does_not_reset_the_failed_source_backoff() {
    let mut test = Fixture::new(Kind::Vector, true).await;
    test.source
        .set_healthy(Response::Bytes(super::super::tile()));
    let mut time = 0;
    let mut requests = 0;
    for delay in [1000, 2000, 4000, 8000, 16000, 30000, 30000] {
        test.frame(time);
        test.receive().await;
        requests += 2;
        assert_eq!(test.source.requests(), requests);
        assert!(
            !test.loaded(),
            "incomplete source coverage retains fallback"
        );
        for offset in [0, 1, delay - 1] {
            test.frame(time + offset);
            test.receive().await;
            assert_eq!(
                test.source.requests(),
                requests,
                "partial success cannot cause a request storm"
            );
        }
        time += delay;
    }
    test.source.set(Response::Bytes(super::super::tile()));
    test.frame(time);
    test.receive().await;
    assert!(test.loaded());
    test.frame(time + 60000);
    test.receive().await;
    assert_eq!(
        test.source.requests(),
        requests + 2,
        "complete success cancels retries"
    );
}
