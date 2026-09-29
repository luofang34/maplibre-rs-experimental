use maplibre::{
    environment::{OffscreenKernel, OffscreenKernelConfig},
    io::{
        apc::{
            AsyncProcedure, AsyncProcedureCall, CallError, Context, Input, IntoMessage, Message,
            SendError,
        },
        scheduler::NopScheduler,
    },
    kernel::KernelBuilder,
    map::Map,
    platform::{http_client::ReqwestHttpClient, ReqwestOffscreenKernelEnvironment},
    render::{builder::RendererBuilder, RenderPlugin},
    sdf::SdfPlugin,
    vector::{DefaultVectorTransferables, VectorPlugin},
};
use maplibre_winit::{WinitEnvironment, WinitMapWindowConfig};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
    vec::IntoIter,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

type Procedure = AsyncProcedure<ReqwestOffscreenKernelEnvironment, Replies>;
pub(super) type Environment =
    WinitEnvironment<NopScheduler, ReqwestHttpClient, ReqwestOffscreenKernelEnvironment, Calls, ()>;

#[derive(Clone, Default)]
pub struct Replies(Arc<Mutex<Vec<Message>>>);
impl Context for Replies {
    fn send_back<T: IntoMessage>(&self, message: T) -> Result<(), SendError> {
        self.0.lock().expect("reply queue").push(message.into());
        Ok(())
    }
}

pub(super) struct Work(Input, Procedure);
impl Work {
    pub(super) async fn execute(&self) -> Vec<Message> {
        let replies = Replies::default();
        (self.1)(
            self.0.clone(),
            replies.clone(),
            ReqwestOffscreenKernelEnvironment::create(OffscreenKernelConfig {
                cache_directory: None,
            }),
        )
        .await
        .expect("actual HTTP vector worker");
        let messages = std::mem::take(&mut *replies.0.lock().expect("completed replies"));
        messages
    }
}

#[derive(Default)]
pub struct Calls {
    pending: RefCell<Vec<Work>>,
    replies: Replies,
}
impl AsyncProcedureCall<ReqwestOffscreenKernelEnvironment> for Calls {
    type Context = Replies;
    type ReceiveIterator<F: FnMut(&Message) -> bool> = IntoIter<Message>;
    fn receive<F: FnMut(&Message) -> bool>(&self, mut filter: F) -> Self::ReceiveIterator<F> {
        self.replies
            .0
            .lock()
            .expect("reply queue")
            .extract_if(.., |message| filter(message))
            .collect::<Vec<_>>()
            .into_iter()
    }
    fn call(&self, input: Input, procedure: Procedure) -> Result<(), CallError> {
        self.pending.borrow_mut().push(Work(input, procedure));
        Ok(())
    }
}
impl Calls {
    pub(super) fn take_one(&self) -> Work {
        let mut pending = self.pending.borrow_mut();
        assert_eq!(
            pending.len(),
            1,
            "one tile admitted by the production scheduler"
        );
        pending.remove(0)
    }
    pub fn deliver(&self, messages: Vec<Message>) {
        self.replies.0.lock().expect("reply queue").extend(messages);
    }
    pub(super) fn pending(&self) -> usize {
        self.pending.borrow().len()
    }
}

pub(super) fn map(config: WinitMapWindowConfig<()>, url: &str) -> Map<Environment> {
    let style = serde_json::from_value(serde_json::json!({
        "version":8,"center":[0,0],"zoom":0,
        "sources":{"source":{"type":"vector","tiles":[format!("{url}/{{z}}/{{x}}/{{y}}")],"maxzoom":0}},
        "layers":[{"id":"land","source":"source","source-layer":"land","type":"fill","paint":{"fill-color":["get","color"]}}]
    })).expect("vector style");
    create_map(
        config,
        style,
        vec![
            Box::new(RenderPlugin),
            Box::new(VectorPlugin::<DefaultVectorTransferables>::default()),
            Box::new(SdfPlugin::<DefaultVectorTransferables>::default()),
        ],
    )
}

pub(super) fn create_map(
    config: WinitMapWindowConfig<()>,
    style: maplibre::style::Style,
    plugins: Vec<Box<dyn maplibre::plugin::Plugin<Environment>>>,
) -> Map<Environment> {
    let kernel = KernelBuilder::new()
        .with_map_window_config(config)
        .with_http_client(ReqwestHttpClient::new::<String>(None))
        .with_scheduler(NopScheduler)
        .with_apc(Calls::default())
        .build()
        .expect("map services");
    Map::new(style, kernel, RendererBuilder::new(), plugins).expect("map bound to resumed window")
}

pub(super) struct Source {
    pub url: String,
    response: Arc<Mutex<(u16, Vec<u8>)>>,
    task: tokio::task::JoinHandle<()>,
}
impl Source {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("local tile server");
        let url = format!("http://{}", listener.local_addr().expect("server address"));
        let response = Arc::new(Mutex::new((503, Vec::new())));
        let current = response.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    headers.push(stream.read_u8().await.expect("request headers"));
                }
                assert!(String::from_utf8_lossy(&headers).starts_with("GET /0/0/0 "));
                let (status, body) = current.lock().expect("HTTP response").clone();
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
            task,
        }
    }
    pub fn tile(&self, color: &str) {
        *self.response.lock().expect("HTTP response") = (200, tile(color));
    }
    pub fn image(&self, value: u8) {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&[128, value, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .expect("PNG");
        *self.response.lock().expect("HTTP response") = (200, bytes);
    }
    pub fn unavailable(&self) {
        *self.response.lock().expect("HTTP response") = (503, Vec::new());
    }
    pub async fn close(self) {
        self.task.abort();
        assert!(self.task.await.expect_err("server canceled").is_cancelled());
    }
}

fn tile(color: &str) -> Vec<u8> {
    use geozero::mvt::{tile, Message, Tile};
    Tile {
        layers: vec![tile::Layer {
            version: 2,
            name: "land".into(),
            extent: Some(4096),
            features: vec![tile::Feature {
                id: Some(1),
                tags: vec![0, 0],
                r#type: Some(tile::GeomType::Polygon as i32),
                geometry: vec![9, 0, 0, 26, 8192, 0, 0, 8192, 8191, 0, 15],
            }],
            keys: vec!["color".into()],
            values: vec![tile::Value {
                string_value: Some(color.into()),
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}
