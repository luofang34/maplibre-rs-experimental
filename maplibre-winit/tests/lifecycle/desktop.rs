use std::{
    cell::RefCell,
    rc::Rc,
    sync::{mpsc, Arc, Weak},
    time::{Duration, Instant},
};

use maplibre::{
    map::MapError,
    window::{HeadedMapWindow, MapWindowConfig, WindowCreateError},
};
use maplibre_winit::{RawWinitWindow, WinitApplication, WinitMapWindowConfig};
use winit::{
    application::ApplicationHandler,
    event::{StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::WindowId,
};

#[path = "desktop/fixture.rs"]
mod fixture;

#[derive(Clone, Debug)]
pub(super) enum Event {
    MetadataBlocked,
    CanceledResponse,
    Presented { generation: u64, size: (u32, u32) },
}

#[derive(Default)]
struct Observed {
    window: Weak<RawWinitWindow>,
    constructed: u64,
}

struct Driver<A> {
    app: A,
    observed: Rc<RefCell<Observed>>,
    release: mpsc::Sender<()>,
    deadline: Instant,
    checked_pending: bool,
    checked_ready: bool,
    complete: bool,
}

pub(super) fn run() {
    let config = WinitMapWindowConfig::<Event>::new("Map lifecycle guard".into());
    assert!(matches!(
        config.create(),
        Err(WindowCreateError::WindowNotBound)
    ));
    let events = EventLoop::<Event>::with_user_event()
        .build()
        .expect("desktop event loop");
    let observed = Rc::new(RefCell::new(Observed::default()));
    let (url, release, server) = fixture::metadata_server(events.create_proxy());
    let state = observed.clone();
    let proxy = events.create_proxy();
    let factory = move |config| {
        let map = fixture::create_map(config, &url, proxy.clone());
        let mut state = state.borrow_mut();
        state.constructed = state.constructed.wrapping_add(1);
        state.window = Arc::downgrade(map.window().handle());
        Ok::<_, MapError>(map)
    };
    let mut driver = Driver {
        app: WinitApplication::new(config, factory, None),
        observed,
        release,
        deadline: Instant::now() + Duration::from_secs(30),
        checked_pending: false,
        checked_ready: false,
        complete: false,
    };
    maplibre::platform::run_multithreaded(async {
        events.run_app(&mut driver).expect("desktop callbacks");
    });
    assert!(
        driver.complete,
        "lifecycle guard reached its final rendered frame"
    );
    assert_eq!(
        driver.observed.borrow().constructed,
        1,
        "resume retains the map and services"
    );
    driver.app.finish().expect("map initialized and rendered");
    server.join().expect("metadata server joined");
    assert_eq!(
        driver.observed.borrow().window.strong_count(),
        0,
        "all owners released"
    );
}

impl<A: ApplicationHandler<Event>> Driver<A> {
    fn owners(&self) -> usize {
        self.observed.borrow().window.strong_count()
    }
    fn cancel_pending(&mut self, active: &ActiveEventLoop) {
        assert_eq!(
            self.owners(),
            2,
            "pending initialization already owns a GPU surface"
        );
        let window = self
            .observed
            .borrow()
            .window
            .upgrade()
            .expect("live window");
        let _requested_size = window.request_inner_size(winit::dpi::PhysicalSize::new(320, 240));
        drop(window);
        self.app.suspended(active);
        self.app.suspended(active);
        assert_eq!(
            self.owners(),
            1,
            "suspend immediately releases the pending surface"
        );
        self.checked_pending = true;
        self.release
            .send(())
            .expect("release the canceled HTTP response");
    }
    fn resume_canceled(&mut self, active: &ActiveEventLoop) {
        assert!(self.checked_pending);
        assert_eq!(
            self.owners(),
            1,
            "late response cannot install a canceled surface"
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
            "a queued redraw cannot poll suspended initialization"
        );
        self.app.resumed(active);
        self.app.resumed(active);
    }
    fn presented(&mut self, active: &ActiveEventLoop, generation: u64, size: (u32, u32)) {
        if generation == 1 && self.checked_ready {
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
            "first frame uses current window extent"
        );
        drop(window);
        assert_eq!(self.owners(), 2, "ready renderer retains one surface owner");
        if generation == 1 && !self.checked_ready {
            self.app.suspended(active);
            self.app.suspended(active);
            assert_eq!(
                self.owners(),
                1,
                "ready suspension immediately releases its surface"
            );
            self.checked_ready = true;
            self.app.resumed(active);
            self.app.resumed(active);
        } else if generation == 2 {
            assert!(self.checked_ready);
            self.app.suspended(active);
            self.app.resumed(active);
            self.app.exiting(active);
            assert_eq!(
                self.owners(),
                0,
                "pending exit releases the window while its handler remains alive"
            );
            self.complete = true;
            active.exit();
        }
    }
}

impl<A: ApplicationHandler<Event>> ApplicationHandler<Event> for Driver<A> {
    fn new_events(&mut self, active: &ActiveEventLoop, cause: StartCause) {
        assert!(
            Instant::now() < self.deadline,
            "lifecycle guard exceeded its event deadline: {cause:?}"
        );
        active.set_control_flow(ControlFlow::WaitUntil(self.deadline));
    }
    fn resumed(&mut self, active: &ActiveEventLoop) {
        self.app.resumed(active);
        self.app.resumed(active);
    }
    fn suspended(&mut self, active: &ActiveEventLoop) {
        self.app.suspended(active);
    }
    fn user_event(&mut self, active: &ActiveEventLoop, event: Event) {
        match event {
            Event::MetadataBlocked => self.cancel_pending(active),
            Event::CanceledResponse => self.resume_canceled(active),
            Event::Presented { generation, size } => self.presented(active, generation, size),
        }
    }
    fn window_event(&mut self, active: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.app.window_event(active, id, event);
    }
    fn exiting(&mut self, active: &ActiveEventLoop) {
        self.app.exiting(active);
    }
}
