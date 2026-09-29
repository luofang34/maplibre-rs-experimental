use cgmath::{Deg, Point2, Vector2, Vector4};
use maplibre::{
    coords::{LatLon, WorldCoords, Zoom},
    map::Map,
    projection::ProjectionType,
    render::{camera::EdgeInsets, eventually::Eventually, view_state::ViewState, RenderPlugin},
    window::{HeadedMapWindow, MapWindow, PhysicalSize},
};
use maplibre_winit::{WinitMapWindow, WinitMapWindowConfig};

use super::{fixture, Environment};

#[path = "camera/cancellation.rs"]
mod cancellation;

pub(super) struct Check {
    map: Map<Environment>,
    expected: ViewState,
    previous_size: PhysicalSize,
    initial_scale_factor: f64,
}

impl Check {
    pub(super) async fn start(config: WinitMapWindowConfig<()>) -> Self {
        let style = serde_json::from_value(serde_json::json!({
            "version":8,"center":[11,47],"zoom":4.5,"pitch":15,"bearing":25,"roll":5,
            "sources":{},"layers":[]
        }))
        .expect("camera style");
        let mut map = fixture::create_map(config, style, vec![Box::new(RenderPlugin)]);
        if std::env::args().any(|arg| arg == "--high-dpi") {
            assert!(
                map.window().scale_factor() > 1.0,
                "this run requires a high-DPI window"
            );
        }
        map.set_max_pitch(Deg(85.0));
        super::initialize(&mut map).await;
        let view = &mut map.context_mut().expect("first renderer").view_state;
        assert_center(view, LatLon::new(47.0, 11.0), 4.5);
        assert_angles(view, 15.0, 25.0, 5.0);
        view.zoom_to(Zoom::new(7.25));
        let center = WorldCoords::from_lat_lon(LatLon::new(48.0, 17.0), view.zoom());
        view.camera_mut().move_to(Point2::new(center.x, center.y));
        view.camera_mut().set_pitch(Deg(72.0));
        view.camera_mut().set_bearing(Deg(-42.0));
        view.camera_mut().set_roll(Deg(17.0));
        view.set_edge_insets(EdgeInsets {
            top: 12.0,
            bottom: 4.0,
            left: 9.0,
            right: 3.0,
        });
        let expected = view.clone();
        let previous_size = map.window().size();
        let initial_scale_factor = map.window().scale_factor();
        map.reset();
        super::initialize(&mut map).await;
        assert_view(&map, &expected);
        assert_unchanged_viewport(&map, &expected, previous_size);
        map.reset();
        map.reset();
        let _requested_size =
            map.window()
                .handle()
                .request_inner_size(winit::dpi::PhysicalSize::new(
                    previous_size.width() / 2,
                    previous_size.height() / 2,
                ));
        Self {
            map,
            expected,
            previous_size,
            initial_scale_factor,
        }
    }

    pub(super) fn was_resized(&self, window: &WinitMapWindow<()>) -> bool {
        window.size() != self.previous_size
    }

    pub(super) async fn finish(mut self) {
        super::initialize(&mut self.map).await;
        assert_view(&self.map, &self.expected);
        super::frame(&mut self.map, 0);
        assert_current_size(&self.map);
        assert_view(&self.map, &self.expected);
        assert_eq!(
            self.map.context().expect("ready").style.center,
            Some([11.0, 47.0])
        );
        let before_cancel = self.map.context().expect("ready").view_state.clone();
        let size = self.map.window().size();
        cancellation::check(&mut self.map).await;
        assert_view(&self.map, &self.expected);
        assert_unchanged_viewport(&self.map, &before_cancel, size);
        self.external_view().await;
        self.write_observation_blocking();
        tracing::info!("camera reset preserves orbit state and uses the resized viewport");
    }

    fn write_observation_blocking(&self) {
        let args: Vec<_> = std::env::args().collect();
        let Some(index) = args.iter().position(|arg| arg == "--camera-observations") else {
            return;
        };
        let path = args.get(index + 1).expect("observation output path");
        let size = self.map.window().size();
        let view = &self.map.context().expect("ready").view_state;
        let observation = serde_json::json!({
            "initial": {
                "scale_factor": self.initial_scale_factor,
                "physical": [self.previous_size.width(), self.previous_size.height()],
                "viewport": [self.expected.width(), self.expected.height()]
            },
            "resized": {
                "scale_factor": self.map.window().scale_factor(),
                "physical": [size.width(), size.height()],
                "viewport": [view.width(), view.height()]
            }
        });
        std::fs::write(path, observation.to_string()).expect("save actual viewport observations");
    }

    async fn external_view(&mut self) {
        let view = &mut self.map.context_mut().expect("ready").view_state;
        let mut eye = view.external_view();
        eye.frustum.left *= 0.2;
        eye.frustum.right *= 1.8;
        view.set_external_view(eye, &ProjectionType::Mercator)
            .expect("valid eye");
        assert!(view.has_external_view());
        let expected = view.clone();
        self.map.reset();
        super::initialize(&mut self.map).await;
        assert_view(&self.map, &expected);
        let view = &self
            .map
            .context()
            .expect("resumed external view")
            .view_state;
        assert!(
            !view.has_external_view(),
            "an external frame must be supplied again"
        );
        assert!(view.external_projection().is_none());
        self.map.set_max_pitch(Deg(40.0));
        self.map.reset();
        super::initialize(&mut self.map).await;
        let view = &self.map.context().expect("updated pitch bound").view_state;
        assert_close(view.camera().max_pitch().0.to_degrees(), 40.0);
        assert_close(view.camera().get_pitch().0.to_degrees(), 40.0);
    }
}

fn assert_unchanged_viewport(map: &Map<Environment>, expected: &ViewState, size: PhysicalSize) {
    assert_eq!(
        map.window().size(),
        size,
        "the first reset does not resize the window"
    );
    let actual = &map.context().expect("same-size resume").view_state;
    assert_eq!(
        (actual.width(), actual.height()),
        (expected.width(), expected.height()),
        "viewport units must survive a reset at scale {}",
        map.window().scale_factor()
    );
    assert_eq!(
        actual.view_projection().0,
        expected.view_projection().0,
        "an unchanged window and camera must retain their visible region"
    );
    for position in [
        LatLon::new(48.0, 17.0),
        LatLon::new(48.0, 17.15),
        LatLon::new(48.08, 17.0),
    ] {
        let before = project_and_unproject(expected, position);
        let after = project_and_unproject(actual, position);
        assert_close(before.x, after.x);
        assert_close(before.y, after.y);
    }
}

fn project_and_unproject(view: &ViewState, position: LatLon) -> Vector2<f64> {
    let world = WorldCoords::from_lat_lon(position, view.zoom());
    let clip = view
        .view_projection()
        .project(Vector4::new(world.x, world.y, 0.0, 1.0));
    let screen = Vector2::new(
        (clip.x / clip.w + 1.0) * view.width() / 2.0,
        (1.0 - clip.y / clip.w) * view.height() / 2.0,
    );
    assert!((0.0..=view.width()).contains(&screen.x));
    assert!((0.0..=view.height()).contains(&screen.y));
    let unprojected = view
        .window_to_world_at_ground(
            &screen,
            &view.inverted_view_projection().expect("invertible camera"),
            true,
        )
        .expect("visible geographic point");
    assert_close(world.x, unprojected.x);
    assert_close(world.y, unprojected.y);
    screen
}

fn assert_current_size(map: &Map<Environment>) {
    let size = map.window().size();
    let logical = size.to_logical(map.window().scale_factor());
    let context = map.context().expect("resumed renderer");
    assert_eq!(context.view_state.width(), f64::from(logical.width()));
    assert_eq!(context.view_state.height(), f64::from(logical.height()));
    assert_eq!(context.renderer.resources.surface.size(), size);
    let Eventually::Initialized(depth) = &context.renderer.resources.depth_texture else {
        panic!("the resumed schedule must allocate frame attachments");
    };
    assert_eq!(depth.texture.width(), size.width());
    assert_eq!(depth.texture.height(), size.height());
}

fn assert_view(map: &Map<Environment>, expected: &ViewState) {
    let actual = &map.context().expect("resumed renderer").view_state;
    assert_close(actual.zoom().value(), expected.zoom().value());
    assert_close(actual.camera().position().x, expected.camera().position().x);
    assert_close(actual.camera().position().y, expected.camera().position().y);
    assert_angles(
        actual,
        expected.camera().get_pitch().0.to_degrees(),
        expected.camera().get_bearing().0.to_degrees(),
        expected.camera().get_roll().0.to_degrees(),
    );
    let a = actual.edge_insets();
    let e = expected.edge_insets();
    assert_eq!(
        [a.top, a.bottom, a.left, a.right],
        [e.top, e.bottom, e.left, e.right]
    );
}

fn assert_center(view: &ViewState, center: LatLon, zoom: f64) {
    assert_close(view.zoom().value(), zoom);
    let expected = WorldCoords::from_lat_lon(center, Zoom::new(zoom));
    assert_close(view.camera().position().x, expected.x);
    assert_close(view.camera().position().y, expected.y);
}

fn assert_angles(view: &ViewState, pitch: f64, bearing: f64, roll: f64) {
    assert_close(view.camera().get_pitch().0.to_degrees(), pitch);
    assert_close(view.camera().get_bearing().0.to_degrees(), bearing);
    assert_close(view.camera().get_roll().0.to_degrees(), roll);
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-8,
        "{actual} differs from {expected}"
    );
}
