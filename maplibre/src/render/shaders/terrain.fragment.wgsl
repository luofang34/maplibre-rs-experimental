struct TerrainTileUniforms {
    transform: mat4x4<f32>,
    dem_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
    dem_unpack: vec4<f32>,
    dem_dim: f32,
    exaggeration: f32,
    skirt_length: f32,
    padding: f32,
    fog_color: vec4<f32>,
    horizon_color: vec4<f32>,
    fog_range: vec4<f32>,
    fog_opacity: vec4<f32>,
};

struct VertexOutput {
    @location(0) tex_coords: vec2<f32>,
    @location(1) horizon_distance: f32,
    @location(2) fog_depth: f32,
    @builtin(position) position: vec4<f32>,
};

@group(1) @binding(0) var<uniform> terrain_tile: TerrainTileUniforms;
@group(1) @binding(2) var drape_texture: texture_2d<f32>;
@group(1) @binding(3) var drape_sampler: sampler;

fn gamma_to_linear(color: vec4<f32>) -> vec4<f32> {
    return pow(color, vec4<f32>(2.2));
}

fn linear_to_gamma(color: vec4<f32>) -> vec4<f32> {
    return pow(color, vec4<f32>(1.0 / 2.2));
}

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.horizon_distance < 0.0 {
        discard;
    }
    let surface = textureSample(drape_texture, drape_sampler, in.tex_coords);
    let ground_blend = terrain_tile.fog_range.z;
    let horizon_blend = terrain_tile.fog_range.w;
    let opacity = terrain_tile.fog_opacity.x;
    let globe = terrain_tile.fog_opacity.y > 0.5;
    // GL JS blends fog only on the flat map, from the ground blend depth outwards, and turns
    // the fog colour into the horizon colour towards the far plane.
    if globe || opacity <= 0.0 || in.fog_depth <= ground_blend {
        return surface;
    }
    let blend_color = smoothstep(
        0.0,
        1.0,
        max((in.fog_depth - horizon_blend) / (1.0 - horizon_blend), 0.0),
    );
    let fog_horizon = mix(
        gamma_to_linear(terrain_tile.fog_color),
        gamma_to_linear(terrain_tile.horizon_color),
        blend_color,
    );
    let factor = max(in.fog_depth - ground_blend, 0.0) / (1.0 - ground_blend);
    return linear_to_gamma(mix(
        gamma_to_linear(surface),
        fog_horizon,
        pow(factor, 2.0) * opacity,
    ));
}
