use std::time::Duration;

use maplibre::window::{HeadedMapWindow, MapWindowConfig};
use maplibre::{
    map::Map,
    render::frame_input::FrameInput,
    vector::{VectorLayerBucket, VectorLayerBucketComponent},
};
use maplibre_winit::{WinitMapWindow, WinitMapWindowConfig};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::WindowId,
};

#[path = "desktop/fixture.rs"]
mod fixture;
use fixture::{Environment, Source};
#[path = "desktop/images.rs"]
mod images;

pub(super) fn run() {
    let mut driver = Driver {
        complete: false,
        window: None,
    };
    EventLoop::new()
        .expect("desktop event loop")
        .run_app(&mut driver)
        .expect("desktop callbacks");
    assert!(driver.complete, "reset checks completed");
}

struct Driver {
    complete: bool,
    window: Option<WinitMapWindow<()>>,
}
impl ApplicationHandler for Driver {
    fn resumed(&mut self, active: &ActiveEventLoop) {
        if self.complete || self.window.is_some() {
            return;
        }
        let config = WinitMapWindowConfig::<()>::new("Map reset reply guard".into());
        let window = config.create_window(active).expect("resumed window");
        window.request_redraw();
        self.window = Some(window);
    }
    fn window_event(&mut self, active: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if !matches!(event, WindowEvent::RedrawRequested) || self.complete {
            return;
        }
        let window = self.window.as_ref().expect("resumed window");
        let config = WinitMapWindowConfig::new("Map reset reply guard".into()).with_window(window);
        let runtime = tokio::runtime::Runtime::new().expect("HTTP runtime");
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), check(config))
                .await
                .expect("reset guard completed before its deadline");
        });
        self.complete = true;
        active.exit();
    }
}

async fn check(config: WinitMapWindowConfig<()>) {
    let source = Source::new().await;
    if std::env::args().any(|arg| arg == "--images-only" || arg == "--dem-only") {
        images::check(config, &source).await;
        source.close().await;
        return;
    }
    let mut map = fixture::map(config.clone(), &source.url);
    initialize(&mut map).await;
    if !std::env::args().any(|arg| arg == "--outcome-only") {
        payloads(&mut map, &source).await;
    }
    outcomes(&mut map, &source).await;
    images::check(config, &source).await;
    source.close().await;
}

async fn initialize(map: &mut Map<Environment>) {
    map.initialize_renderer()
        .await
        .expect("actual window renderer");
    let window = map
        .kernel()
        .map_window_config()
        .create()
        .expect("live window");
    let renderer = &mut map.context_mut().expect("ready map").renderer;
    // A compositor may skip window frames while this callback drives worker delivery.
    renderer.resources.surface = maplibre::render::resource::Surface::from_image(
        &renderer.device,
        &renderer.adapter,
        &window,
        &renderer.settings,
    );
}

fn frame(map: &mut Map<Environment>, millis: u64) {
    map.context_mut()
        .expect("ready map")
        .world
        .resources
        .get_or_init_mut::<FrameInput>()
        .timestamp = Duration::from_millis(millis);
    map.run_schedule()
        .expect("production request/populate/render");
}

async fn reset(map: &mut Map<Environment>) {
    map.reset();
    assert!(!map.is_initialized());
    initialize(map).await;
    frame(map, 0);
    assert_eq!(map.kernel().apc().pending(), 1, "same coordinate requested");
}

async fn payloads(map: &mut Map<Environment>, source: &Source) {
    source.tile("#ff0000");
    frame(map, 0);
    let old = map.kernel().apc().take_one();
    let delayed = old.execute().await;
    reset(map).await;
    let current = map.kernel().apc().take_one();
    map.kernel().apc().deliver(delayed);
    frame(map, 1);
    let component = component(map);
    assert!(
        component.layers.is_empty(),
        "old payload cannot fill a new world"
    );
    assert!(!component.done, "old completion cannot finish a new tile");

    source.tile("#00ff00");
    map.kernel().apc().deliver(current.execute().await);
    frame(map, 2);
    assert_green(map);
    source.tile("#ff0000");
    map.kernel().apc().deliver(old.execute().await);
    frame(map, 3);
    assert_green(map);
}

async fn outcomes(map: &mut Map<Environment>, source: &Source) {
    reset(map).await;
    source.unavailable();
    let delayed = map.kernel().apc().take_one().execute().await;
    reset(map).await;
    map.kernel().apc().deliver(delayed);
    frame(map, 1);
    frame(map, 2000);
    assert_eq!(
        map.kernel().apc().pending(),
        1,
        "an old retry outcome cannot release the new request and schedule a duplicate"
    );
    source.tile("#00ff00");
    let current = map.kernel().apc().take_one();
    map.kernel().apc().deliver(current.execute().await);
    frame(map, 2001);
    assert_green(map);
    frame(map, 4000);
    assert_eq!(
        map.kernel().apc().pending(),
        0,
        "successful request stays settled"
    );
}

fn component(map: &Map<Environment>) -> &VectorLayerBucketComponent {
    map.context()
        .expect("ready map")
        .world
        .tiles
        .query::<&VectorLayerBucketComponent>(Default::default())
        .expect("requested tile")
}

fn assert_green(map: &Map<Environment>) {
    let component = component(map);
    assert!(component.done && !component.failed);
    assert_eq!(component.layers.len(), 1);
    let VectorLayerBucket::AvailableLayer(layer) = &component.layers[0] else {
        panic!("successful vector geometry");
    };
    assert!(!layer.buffer.buffer.indices.is_empty());
    assert_eq!(layer.feature_colors, [[0.0, 1.0, 0.0, 1.0]]);
    assert!(maplibre::vector::geometry_uploaded(
        Default::default(),
        &map.context().expect("ready map").world
    ));
}
