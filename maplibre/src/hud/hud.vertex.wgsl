struct HudFrame {
    viewport_and_panel: vec4<f32>,
    angles_and_interaction: vec4<f32>,
    state_flags: vec4<u32>,
    selection_rect: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: HudFrame;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) logical_pos: vec2<f32>,
    @location(1) @interpolate(flat) quad_kind: u32,
};

fn quad_corner(index: u32) -> vec2<f32> {
    switch index {
        case 0u: { return vec2<f32>(0.0, 0.0); }
        case 1u: { return vec2<f32>(1.0, 0.0); }
        case 2u: { return vec2<f32>(0.0, 1.0); }
        case 3u: { return vec2<f32>(0.0, 1.0); }
        case 4u: { return vec2<f32>(1.0, 0.0); }
        default: { return vec2<f32>(1.0, 1.0); }
    }
}

@vertex
fn main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let viewport = max(frame.viewport_and_panel.xy, vec2<f32>(1.0, 1.0));
    let quad_id = vertex_index / 6u;
    let corner = quad_corner(vertex_index % 6u);

    var out: VertexOutput;
    out.quad_kind = quad_id;

    if quad_id == 0u {
        let panel_min = frame.viewport_and_panel.zw - vec2<f32>(8.0, 8.0);
        let panel_max = frame.viewport_and_panel.zw + vec2<f32>(38.0 + 8.0, 186.0 + 8.0);
        let pos = mix(panel_min, panel_max, corner);
        let ndc = vec2<f32>(
            pos.x / viewport.x * 2.0 - 1.0,
            1.0 - pos.y / viewport.y * 2.0,
        );
        out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
        out.logical_pos = pos;
        return out;
    }

    if frame.state_flags.y == 0u {
        out.clip_position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        out.logical_pos = vec2<f32>(0.0, 0.0);
        return out;
    }

    let sel_min = frame.selection_rect.xy - vec2<f32>(3.0, 3.0);
    let sel_max = frame.selection_rect.zw + vec2<f32>(3.0, 3.0);
    let pos = mix(sel_min, sel_max, corner);
    let ndc = vec2<f32>(
        pos.x / viewport.x * 2.0 - 1.0,
        1.0 - pos.y / viewport.y * 2.0,
    );
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.logical_pos = pos;
    return out;
}
