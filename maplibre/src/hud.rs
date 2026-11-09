//! Cross-platform GPU navigation HUD overlay and tactical selection rectangle.
//!
//! Renders a compact floating control capsule in the top-right safe area of the viewport
//! (3D compass with north-lock, `3D/2D` + `AUTO/MAN` tilt toggle, `+`/`-` zoom buttons, `Home`
//! reset, and `MAP/RTS` pointer profile switch), plus an optional rubber-band selection box
//! when dragging a box selection in RTS or `Shift`+drag mode.

use bytemuck_derive::{Pod, Zeroable};
use cgmath::Vector2;

use crate::{
    context::MapContext,
    environment::Environment,
    plugin::Plugin,
    render::{
        camera::EdgeInsets,
        eventually::{Eventually, Eventually::Initialized},
        resource::{FragmentState, RenderPipeline, TilePipeline, TilePipelineOptions, VertexState},
        shaders::Shader,
        tracked_pass::TrackedRenderPass,
        RenderStageLabel,
    },
    tcs::{
        system::{SystemError, SystemResult},
        world::World,
    },
};

/// Logical pixel width of the floating HUD capsule.
pub const HUD_WIDTH: f64 = 38.0;
/// Logical pixel height of the floating HUD capsule.
pub const HUD_HEIGHT: f64 = 186.0;
/// Right margin from the viewport safe area in logical pixels.
pub const HUD_MARGIN_RIGHT: f64 = 12.0;
/// Top margin from the viewport safe area in logical pixels.
pub const HUD_MARGIN_TOP: f64 = 12.0;

/// Interactive slots inside the floating navigation HUD capsule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudButton {
    /// 3D compass needle: click resets north or toggles north-lock; drag orbits bearing/pitch.
    Compass = 0,
    /// Cycles `3D AUTO` -> `2D` -> `3D AUTO` (or restores `3D AUTO` from `3D MAN`).
    Mode3D = 1,
    /// Smoothly zooms in by one level around the viewport center.
    ZoomIn = 2,
    /// Smoothly zooms out by one level around the viewport center.
    ZoomOut = 3,
    /// Resets the camera to the style's home view and restores `Auto-Tilt`.
    Home = 4,
    /// Switches between `Map` (left-drag pan) and `RTS` (left-drag box select, right-drag pan).
    PointerMode = 5,
}

impl HudButton {
    fn slot_index(self) -> f32 {
        match self {
            Self::Compass => 0.0,
            Self::Mode3D => 1.0,
            Self::ZoomIn => 2.0,
            Self::ZoomOut => 3.0,
            Self::Home => 4.0,
            Self::PointerMode => 5.0,
        }
    }
}

/// Shared state between the input controller and the GPU HUD renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavigationHudState {
    /// Whether the HUD overlay is drawn.
    pub visible: bool,
    /// Whether oblique 3D pitch is permitted (`true` = 3D, `false` = 2D overhead).
    pub three_dimensional: bool,
    /// Whether zoom automatically couples with camera tilt (`Auto-Tilt`).
    pub auto_tilt: bool,
    /// Whether bearing is locked to North (`0°`).
    pub north_locked: bool,
    /// Whether RTS pointer semantics (`Left-drag` box select, `Right-drag` pan, edge scroll) are active.
    pub rts_mode: bool,
    /// Multiplier on scroll-wheel / trackpad zoom rate (`1.0` = default).
    pub zoom_sensitivity: f64,
    /// Screen-edge scrolling speed in logical pixels per second.
    pub edge_scroll_speed_px_per_sec: f64,
    /// Whether screen-edge scrolling is enabled even in standard Map pointer profile.
    pub edge_scroll_enabled: bool,
    /// Button currently under the pointer, if any.
    pub hovered: Option<HudButton>,
    /// Button currently held down by the pointer, if any.
    pub pressed: Option<HudButton>,
    /// Active box-selection rectangle `[min_x, min_y, max_x, max_y]` in logical pixels.
    pub selection_box: Option<[f32; 4]>,
}

impl Default for NavigationHudState {
    fn default() -> Self {
        Self {
            visible: false,
            three_dimensional: true,
            auto_tilt: false,
            north_locked: false,
            rts_mode: false,
            zoom_sensitivity: 1.0,
            edge_scroll_speed_px_per_sec: 1100.0,
            edge_scroll_enabled: false,
            hovered: None,
            pressed: None,
            selection_box: None,
        }
    }
}

impl NavigationHudState {
    /// Top-left logical pixel coordinates `(x, y)` of the HUD panel for the given viewport and insets.
    pub fn panel_origin(
        viewport_width: f64,
        _viewport_height: f64,
        insets: &EdgeInsets,
    ) -> Vector2<f64> {
        let x = (viewport_width - insets.right - HUD_MARGIN_RIGHT - HUD_WIDTH).max(8.0);
        let y = (insets.top + HUD_MARGIN_TOP).max(8.0);
        Vector2::new(x, y)
    }

    /// Tests whether a logical window coordinate hits one of the HUD capsule buttons.
    pub fn hit_test(
        logical_pos: Vector2<f64>,
        viewport_width: f64,
        viewport_height: f64,
        insets: &EdgeInsets,
    ) -> Option<HudButton> {
        let origin = Self::panel_origin(viewport_width, viewport_height, insets);
        let rel = logical_pos - origin;
        if rel.x < 0.0 || rel.x > HUD_WIDTH || rel.y < 0.0 || rel.y > HUD_HEIGHT {
            return None;
        }
        if rel.y < 40.0 {
            Some(HudButton::Compass)
        } else if rel.y < 74.0 {
            Some(HudButton::Mode3D)
        } else if rel.y < 104.0 {
            Some(HudButton::ZoomIn)
        } else if rel.y < 133.0 {
            Some(HudButton::ZoomOut)
        } else if rel.y < 160.0 {
            Some(HudButton::Home)
        } else {
            Some(HudButton::PointerMode)
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct HudFrame {
    viewport_and_panel: [f32; 4],
    angles_and_interaction: [f32; 4],
    state_flags: [u32; 4],
    selection_rect: [f32; 4],
}

/// Render pipeline for the navigation HUD overlay.
pub struct NavigationHudPipeline(pub wgpu::RenderPipeline);

/// GPU uniform buffer and bind group for the HUD frame state.
#[derive(Default)]
pub struct NavigationHudGpu {
    frame: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    drawn: bool,
    vertex_count: u32,
}

struct HudShader {
    format: wgpu::TextureFormat,
}

impl Shader for HudShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: include_str!("hud/hud.vertex.wgsl"),
            entry_point: "main",
            buffers: Vec::new(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("hud/hud.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

/// Plugin that draws the cross-platform navigation HUD at the end of the translucent pass.
#[derive(Default)]
pub struct NavigationHudPlugin;

impl<E: Environment> Plugin<E> for NavigationHudPlugin {
    fn build(
        &self,
        schedule: &mut crate::schedule::Schedule,
        _kernel: std::rc::Rc<crate::kernel::Kernel<E>>,
        world: &mut World,
        _renderer: &mut crate::render::Renderer,
    ) -> Result<(), crate::plugin::PluginError> {
        world
            .resources
            .insert(Eventually::<NavigationHudPipeline>::Uninitialized);
        schedule.add_system_to_stage(RenderStageLabel::Prepare, resource_system)?;
        schedule.add_system_to_stage(RenderStageLabel::Queue, queue_system)?;
        if let Some(hooks) = world.resources.get_mut::<crate::render::PassDrawHooks>() {
            hooks.register(crate::render::PassHookPoint::TranslucentPassEnd, draw);
        } else {
            let mut hooks = crate::render::PassDrawHooks::new();
            hooks.register(crate::render::PassHookPoint::TranslucentPassEnd, draw);
            world.resources.insert(hooks);
        }
        Ok(())
    }
}

fn resource_system(
    MapContext {
        world, renderer, ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(pipeline) = world
        .resources
        .get_mut::<Eventually<NavigationHudPipeline>>()
    else {
        return Err(SystemError::Dependencies);
    };
    let device = &renderer.device;
    let settings = renderer.settings;
    let surface = &renderer.resources.surface;
    pipeline.initialize(|| {
        let shader = HudShader {
            format: surface.surface_format(),
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("navigation hud frame layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        NavigationHudPipeline(
            TilePipeline::new(
                "navigation_hud_pipeline".into(),
                settings,
                shader.describe_vertex(),
                shader.describe_fragment(),
                TilePipelineOptions {
                    depth_stencil_enabled: true,
                    update_stencil: false,
                    debug_stencil: true,
                    wireframe: false,
                    multisampling: surface.is_multisampling_supported(settings.msaa),
                    textured: false,
                },
            )
            .describe_render_pipeline()
            .initialize_with_prefix_layouts(device, &[&frame_layout]),
        )
    });
    Ok(())
}

fn queue_system(
    MapContext {
        world,
        view_state,
        renderer,
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(Initialized(NavigationHudPipeline(pipeline))) =
        world.resources.get::<Eventually<NavigationHudPipeline>>()
    else {
        return Ok(());
    };
    let hud_state = world
        .resources
        .get::<NavigationHudState>()
        .copied()
        .unwrap_or_default();
    let frame_layout = pipeline.get_bind_group_layout(0);
    let gpu = world.resources.get_or_init_mut::<NavigationHudGpu>();
    gpu.drawn = hud_state.visible;
    if !hud_state.visible {
        return Ok(());
    }

    let width = view_state.width();
    let height = view_state.height();
    let origin = NavigationHudState::panel_origin(width, height, view_state.edge_insets());
    let mut flags = 0u32;
    if hud_state.three_dimensional {
        flags |= 1;
    }
    if hud_state.auto_tilt {
        flags |= 2;
    }
    if hud_state.north_locked {
        flags |= 4;
    }
    if hud_state.rts_mode {
        flags |= 8;
    }
    let selection_active = u32::from(hud_state.selection_box.is_some());
    gpu.vertex_count = if selection_active != 0 { 12 } else { 6 };

    let frame = HudFrame {
        viewport_and_panel: [
            width as f32,
            height as f32,
            origin.x as f32,
            origin.y as f32,
        ],
        angles_and_interaction: [
            view_state.camera().get_bearing().0 as f32,
            view_state.camera().get_pitch().0 as f32,
            hud_state.hovered.map_or(-1.0, HudButton::slot_index),
            hud_state.pressed.map_or(-1.0, HudButton::slot_index),
        ],
        state_flags: [flags, selection_active, 0, 0],
        selection_rect: hud_state.selection_box.unwrap_or([0.0; 4]),
    };

    let (buffer, _) = gpu.frame.get_or_insert_with(|| {
        let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("navigation hud frame"),
            size: std::mem::size_of::<HudFrame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = renderer
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("navigation hud bind group"),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
        (buffer, bind_group)
    });
    renderer
        .queue
        .write_buffer(buffer, 0, bytemuck::bytes_of(&frame));
    Ok(())
}

/// Draws the navigation HUD at the end of the translucent pass.
pub fn draw<'w>(pass: &mut TrackedRenderPass<'w>, world: &'w World) {
    let Some((Initialized(NavigationHudPipeline(pipeline)), gpu)) = world
        .resources
        .query::<(&Eventually<NavigationHudPipeline>, &NavigationHudGpu)>()
    else {
        return;
    };
    let Some((_, bind_group)) = gpu.frame.as_ref().filter(|_| gpu.drawn) else {
        return;
    };
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..gpu.vertex_count, 0..1);
}
