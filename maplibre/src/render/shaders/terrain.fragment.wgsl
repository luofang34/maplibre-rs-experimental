struct TerrainTileUniforms {
    transform: mat4x4<f32>,
    dem_matrix: mat4x4<f32>,
    drape_matrix: mat4x4<f32>,
    tile_mercator_coords: vec4<f32>,
    dem_unpack: vec4<f32>,
    dem_dim: f32,
    exaggeration: f32,
    skirt_length: f32,
    relief_strength: f32,
    fog_color: vec4<f32>,
    horizon_color: vec4<f32>,
    fog_range: vec4<f32>,
    fog_opacity: vec4<f32>,
    surface_color: vec4<f32>,
    fog_position: vec4<f32>,
    edge_heights: array<vec4<f32>, 128>,
    edge_last: vec4<f32>,
};

struct VertexOutput {
    @location(0) tex_coords: vec2<f32>,
    @location(1) horizon_distance: f32,
    @location(2) eye_depth: f32,
    @location(3) surface_normal: vec3<f32>,
    @location(4) camera_relative_position: vec3<f32>,
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
    let up_normal = normalize(in.surface_normal);
    let light = normalize(vec3<f32>(-0.5, 0.5, 1.0));
    let relief = 1.0 + terrain_tile.relief_strength * (dot(up_normal, light) - light.z);
    if in.horizon_distance < 0.0 {
        discard;
    }
    let drape_uv = (terrain_tile.drape_matrix * vec4<f32>(in.tex_coords, 0.0, 1.0)).xy;
    let draped = select(textureSample(drape_texture, drape_sampler, drape_uv),
        terrain_tile.surface_color, terrain_tile.fog_opacity.z > 0.5);
    let surface = vec4<f32>(draped.rgb * relief, draped.a);
    let ground_blend = terrain_tile.fog_range.z;
    let horizon_blend = terrain_tile.fog_range.w;
    let opacity = terrain_tile.fog_opacity.x;
    let globe = terrain_tile.fog_opacity.y > 0.5;
    // GL JS projects the vertex with a near plane at the map center's distance and reads the
    // depth: 0 at that distance, 1 at the far plane, and held there beyond it.
    let near = terrain_tile.fog_range.x;
    let far = terrain_tile.fog_range.y;
    let eye_depth = max(select(in.eye_depth, length(in.camera_relative_position),
        terrain_tile.fog_opacity.w > 0.5), 1e-6);
    let fog_depth = clamp(
        far * (eye_depth - near) / (eye_depth * max(far - near, 1e-6)),
        0.0,
        1.0,
    );
    // GL JS blends fog only on the flat map, from the ground blend depth outwards, and turns
    // the fog colour into the horizon colour towards the far plane.
    if globe || opacity <= 0.0 || fog_depth <= ground_blend {
        return surface;
    }
    let blend_color = smoothstep(
        0.0,
        1.0,
        max((fog_depth - horizon_blend) / (1.0 - horizon_blend), 0.0),
    );
    let fog_horizon = mix(
        gamma_to_linear(terrain_tile.fog_color),
        gamma_to_linear(terrain_tile.horizon_color),
        blend_color,
    );
    let factor = max(fog_depth - ground_blend, 0.0) / (1.0 - ground_blend);
    return linear_to_gamma(mix(
        gamma_to_linear(surface),
        fog_horizon,
        pow(factor, 2.0) * opacity,
    ));
}
