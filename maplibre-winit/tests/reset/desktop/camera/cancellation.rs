use maplibre::map::Map;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

use super::Environment;

pub(super) async fn check(map: &mut Map<Environment>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("metadata listener");
    let url = format!(
        "http://{}",
        listener.local_addr().expect("metadata address")
    );
    let body = serde_json::json!({"tilejson":"3.0.0","tiles":[format!("{url}/{{z}}/{{x}}/{{y}}")]})
        .to_string();
    let (entered, observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("first metadata request");
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).await.expect("first headers") > 0);
        entered.send(()).expect("observer alive");
        released.await.expect("initialization canceled");
        drop(stream);
        let (mut stream, _) = listener.accept().await.expect("metadata retry");
        assert!(stream.read(&mut request).await.expect("retry headers") > 0);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("metadata response");
    });
    map.context_mut().expect("ready").style.sources.insert(
        "metadata".into(),
        serde_json::from_value(serde_json::json!({"type":"vector","url":url}))
            .expect("metadata source"),
    );
    map.reset();
    let mut initialization = Box::pin(map.initialize_renderer());
    tokio::select! {
        result = &mut initialization => panic!("metadata should be blocked: {result:?}"),
        result = observed => result.expect("metadata request reached the server"),
    }
    drop(initialization);
    map.reset();
    release
        .send(())
        .expect("release canceled metadata connection");
    super::super::initialize(map).await;
    server.await.expect("metadata server completed");
}
