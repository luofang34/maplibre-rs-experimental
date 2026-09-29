#![allow(clippy::expect_used, clippy::panic)]

use maplibre::{map::MapError, window::HeadedMapWindow};
use maplibre_winit::{WinitApplication, WinitMapWindowConfig};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::*;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::web::EventLoopExtWebSys,
    window::WindowId,
};

#[path = "lifecycle/fixture.rs"]
mod fixture;
use fixture::{Event, GateClient, Observed};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn pending_and_ready_surfaces_release_on_suspend_and_render_on_resume() {
    let canvas = create_canvas();
    let observed = Rc::new(RefCell::new(Observed::default()));
    let result = observed.clone();
    let finished = js_sys::Promise::new(&mut move |resolve, _reject| {
        let events = EventLoop::<Event>::with_user_event()
            .build()
            .expect("browser loop");
        let proxy = events.create_proxy();
        let client = GateClient::new(proxy.clone());
        let config = WinitMapWindowConfig::new("lifecycle-test".into());
        let state = result.clone();
        let factory_client = client.clone();
        let app = WinitApplication::new(
            config,
            move |config| {
                let map = fixture::create_map(config, factory_client.clone(), proxy.clone());
                let mut state = state.borrow_mut();
                state.window = Arc::downgrade(map.window().handle());
                state.constructed = state.constructed.wrapping_add(1);
                Ok::<_, MapError>(map)
            },
            None,
        );
        events.spawn_app(Driver {
            app,
            client,
            observed: result.clone(),
            resolve,
        });
    });
    wasm_bindgen_futures::JsFuture::from(finished)
        .await
        .expect("exited lifecycle loop");
    assert!(observed.borrow().complete, "resumed map presented a frame");
    assert_eq!(
        observed.borrow().constructed,
        1,
        "map and worker services survive suspend"
    );
    assert_eq!(
        observed.borrow().window.strong_count(),
        0,
        "exiting releases every window owner"
    );
    canvas.remove();
}

fn create_canvas() -> web_sys::HtmlCanvasElement {
    let document = web_sys::window()
        .expect("window")
        .document()
        .expect("document");
    let canvas = document
        .create_element("canvas")
        .expect("canvas")
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .expect("canvas element");
    canvas.set_id("lifecycle-test");
    canvas.set_width(32);
    canvas.set_height(32);
    document
        .body()
        .expect("body")
        .append_child(&canvas)
        .expect("attached canvas");
    canvas
}

struct Driver<A> {
    app: A,
    client: GateClient,
    observed: Rc<RefCell<Observed>>,
    resolve: js_sys::Function,
}
impl<A: ApplicationHandler<Event>> Driver<A> {
    fn owners(&self) -> usize {
        self.observed.borrow().window.strong_count()
    }
    fn blocked(&mut self, active: &ActiveEventLoop) {
        assert_eq!(self.owners(), 2, "initializing future owns its surface");
        let window = self
            .observed
            .borrow()
            .window
            .upgrade()
            .expect("live window");
        let _requested_size = window.request_inner_size(winit::dpi::PhysicalSize::new(48, 40));
        drop(window);
        self.app.suspended(active);
        self.app.suspended(active);
        assert_eq!(self.owners(), 1, "pending surface released synchronously");
        self.client.release();
    }
    fn late_wake(&mut self, active: &ActiveEventLoop) {
        assert_eq!(
            self.owners(),
            1,
            "late initialization wake cannot recreate a surface"
        );
        let id = self
            .observed
            .borrow()
            .window
            .upgrade()
            .expect("live window")
            .id();
        self.app
            .window_event(active, id, WindowEvent::RedrawRequested);
        assert_eq!(
            self.owners(),
            1,
            "suspended redraw cannot poll a canceled future"
        );
        self.app.resumed(active);
        self.app.resumed(active);
    }
    fn presented(&mut self, active: &ActiveEventLoop, generation: u64, size: (u32, u32)) {
        if generation == 1 && self.observed.borrow().ready_suspended {
            return;
        }
        let window = self
            .observed
            .borrow()
            .window
            .upgrade()
            .expect("live window");
        let actual = window.inner_size();
        assert_eq!(
            size,
            (actual.width, actual.height),
            "resumed frame uses current size"
        );
        drop(window);
        assert_eq!(self.owners(), 2, "ready map owns one surface");
        if generation == 1 {
            self.app.suspended(active);
            self.app.suspended(active);
            assert_eq!(self.owners(), 1, "ready surface released synchronously");
            self.observed.borrow_mut().ready_suspended = true;
            self.app.resumed(active);
            self.app.resumed(active);
        } else {
            assert_eq!(generation, 2);
            self.app.suspended(active);
            self.app.resumed(active);
            self.app.exiting(active);
            assert_eq!(
                self.owners(),
                0,
                "pending exit releases the window before handler drop"
            );
            self.observed.borrow_mut().complete = true;
            active.exit();
        }
    }
}
impl<A: ApplicationHandler<Event>> ApplicationHandler<Event> for Driver<A> {
    fn resumed(&mut self, active: &ActiveEventLoop) {
        self.app.resumed(active);
        self.app.resumed(active);
    }
    fn suspended(&mut self, active: &ActiveEventLoop) {
        self.app.suspended(active);
    }
    fn user_event(&mut self, active: &ActiveEventLoop, event: Event) {
        match event {
            Event::Blocked => self.blocked(active),
            Event::LateWake => self.late_wake(active),
            Event::Presented { generation, size } => self.presented(active, generation, size),
        }
    }
    fn window_event(&mut self, active: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.app.window_event(active, id, event);
    }
    fn exiting(&mut self, active: &ActiveEventLoop) {
        self.app.exiting(active);
        self.resolve.call0(&JsValue::NULL).expect("exiting signal");
    }
}

#[wasm_bindgen_test]
async fn public_spawn_map_resolves_after_renderer_and_plugins_initialize() {
    let canvas = create_canvas();
    let config = WinitMapWindowConfig::new("lifecycle-test".into());
    let events = maplibre_winit::WinitEventLoop::new(&config).expect("browser event loop");
    let ready = Rc::new(std::cell::Cell::new(false));
    let observer = ready.clone();
    let mut exit_callback = None;
    let exited = js_sys::Promise::new(&mut |resolve, _reject| exit_callback = Some(resolve));
    let on_exit = exit_callback.expect("exit resolver");
    events
        .spawn_map(
            config,
            move |config| {
                Ok::<_, MapError>(fixture::startup_map(
                    config,
                    observer.clone(),
                    on_exit.clone(),
                ))
            },
            Some(0),
        )
        .await
        .expect("public startup resolves");
    assert!(
        ready.get(),
        "startup completion follows actual plugin initialization"
    );
    wasm_bindgen_futures::JsFuture::from(exited)
        .await
        .expect("host exited");
    canvas.remove();
}
