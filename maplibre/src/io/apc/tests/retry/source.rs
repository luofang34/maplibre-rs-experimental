use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use image::ImageEncoder;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

#[derive(Clone)]
pub(super) enum Response {
    Status(u16),
    Image,
    Corrupt,
    Bytes(Vec<u8>),
    Disconnect,
}

#[derive(Default)]
pub(super) struct Gate {
    blocked: AtomicBool,
    pub entered: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}

pub(super) struct Source {
    pub url: String,
    response: Arc<Mutex<Response>>,
    healthy: Arc<Mutex<Response>>,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
    gate: Arc<Gate>,
}

impl Source {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("local source");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let response = Arc::new(Mutex::new(Response::Status(503)));
        let healthy = Arc::new(Mutex::new(Response::Image));
        let healthy_reply = healthy.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replies = response.clone();
        let observed = requests.clone();
        let gate = Arc::new(Gate::default());
        let worker_gate = gate.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut bytes = [0; 4096];
                let read = stream.read(&mut bytes).await.expect("request headers");
                let request = String::from_utf8_lossy(&bytes[..read]).into_owned();
                observed.lock().expect("request log").push(request.clone());
                if request.contains("/unstable/")
                    && worker_gate.blocked.swap(false, Ordering::SeqCst)
                {
                    worker_gate.entered.notify_one();
                    worker_gate.release.notified().await;
                }
                let response = if request.contains("/healthy/") {
                    healthy_reply.lock().expect("healthy response").clone()
                } else {
                    replies.lock().expect("response").clone()
                };
                if matches!(response, Response::Disconnect) {
                    continue;
                }
                let (status, body) = match response {
                    Response::Status(status) => (status, Vec::new()),
                    Response::Image => (200, png()),
                    Response::Corrupt => (200, vec![255]),
                    Response::Bytes(bytes) => (200, bytes),
                    Response::Disconnect => unreachable!(),
                };
                let headers = format!(
                    "HTTP/1.1 {status} test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(headers.as_bytes())
                    .await
                    .expect("response headers");
                stream.write_all(&body).await.expect("response body");
            }
        });
        Self {
            url,
            response,
            healthy,
            requests,
            task,
            gate,
        }
    }
    pub fn block_unstable(&self) -> Arc<Gate> {
        self.gate.blocked.store(true, Ordering::SeqCst);
        self.gate.clone()
    }
    pub fn set_healthy(&self, response: Response) {
        *self.healthy.lock().expect("healthy response") = response;
    }
    pub fn set(&self, response: Response) {
        *self.response.lock().expect("response") = response;
    }
    pub fn requests(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[128, 100, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .expect("PNG");
    bytes
}
