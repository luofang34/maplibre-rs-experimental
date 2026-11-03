// @include daylight.wgsl.part

struct TerrainTileUniforms {
    transform: mat4x4<f32>,
    globe_transform: mat4x4<f32>,
    globe_origin: vec4<f32>,
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
    daylight: vec4<f32>,
    twilight: vec4<f32>,
    moonlight: vec4<f32>,
    procedural_relief: vec4<f32>,
    procedural_grid: vec4<f32>,
    edge_index: vec4<u32>,
};

struct VertexOutput {
    @location(0) tex_coords: vec2<f32>,
    @location(1) eye_depth: f32,
    @location(2) surface_normal: vec3<f32>,
    @location(3) camera_relative_position: vec3<f32>,
    @location(4) polar_cap: f32,
    @location(5) globe_normal: vec3<f32>,
    @location(6) surface_elevation: f32,
    @builtin(position) position: vec4<f32>,
};

@group(1) @binding(0) var<uniform> terrain_tile: TerrainTileUniforms;
@group(1) @binding(2) var drape_texture: texture_2d<f32>;
@group(1) @binding(3) var drape_sampler: sampler;

fn gamma_to_linear(color: vec4<f32>) -> vec4<f32> {
    return pow(color, vec4<f32>(2.2));
}

// The ground under the host's sun, as `daylight_weights` turns it: below the twilight band a
// night that is also slightly desaturated, so night reads as night and the map stays legible.
// Without a host sun the colour is unchanged, so the ground is drawn as it is without one.
fn daylight(color: vec3<f32>, globe_normal: vec3<f32>) -> vec3<f32> {
    if terrain_tile.daylight.w < 0.5 {
        return color;
    }
    let weights = daylight_weights(
        normalize(globe_normal),
        terrain_tile.daylight.xyz,
        terrain_tile.twilight,
        terrain_tile.moonlight,
    );
    let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    let night = mix(color, vec3<f32>(luminance), 0.35) * NIGHT_TINT * weights.y;
    return mix(night, color, weights.x);
}

fn linear_to_gamma(color: vec4<f32>) -> vec4<f32> {
    return pow(color, vec4<f32>(1.0 / 2.2));
}

fn unpack_rgba8(packed: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(packed & 0xffu),
        f32((packed >> 8u) & 0xffu),
        f32((packed >> 16u) & 0xffu),
        f32((packed >> 24u) & 0xffu),
    ) / 255.0;
}

fn hypsometric_color(elev_m: f32) -> vec3<f32> {
    let c0 = vec3<f32>(0.18, 0.27, 0.28);
    let c1 = vec3<f32>(0.24, 0.36, 0.24);
    let c2 = vec3<f32>(0.42, 0.45, 0.26);
    let c3 = vec3<f32>(0.55, 0.44, 0.28);
    let c4 = vec3<f32>(0.52, 0.39, 0.31);
    let c5 = vec3<f32>(0.82, 0.83, 0.85);
    if elev_m < 0.0 {
        return mix(c0, c1, clamp((elev_m + 450.0) / 450.0, 0.0, 1.0));
    }
    if elev_m < 500.0 {
        return mix(c1, c2, elev_m / 500.0);
    }
    if elev_m < 1500.0 {
        return mix(c2, c3, (elev_m - 500.0) / 1000.0);
    }
    if elev_m < 3000.0 {
        return mix(c3, c4, (elev_m - 1500.0) / 1500.0);
    }
    return mix(c4, c5, clamp((elev_m - 3000.0) / 1800.0, 0.0, 1.0));
}

fn procedural_surface(
    in: VertexOutput,
    up_normal: vec3<f32>,
    light: vec3<f32>,
    uv_fw: vec2<f32>,
) -> vec4<f32> {
    var base = select(
        terrain_tile.surface_color.rgb,
        hypsometric_color(in.surface_elevation),
        terrain_tile.procedural_relief.y > 0.5,
    );
    if terrain_tile.procedural_grid.z > 0.5 {
        let rel_m = in.surface_elevation - terrain_tile.procedural_relief.w;
        if rel_m >= -30.5 {
            base = mix(base, vec3<f32>(0.88, 0.14, 0.12), 0.82);
        } else if rel_m >= -305.0 {
            base = mix(base, vec3<f32>(0.92, 0.62, 0.08), 0.72);
        }
    }
    let slope = clamp(terrain_tile.procedural_relief.z, 0.0, 1.0);
    let shade = clamp(1.0 + slope * (dot(up_normal, light) - light.z * 0.65), 0.25, 1.45);
    var rgb = base * shade;
    let spacing = terrain_tile.procedural_grid.x;
    let grid_opacity = clamp(terrain_tile.procedural_grid.y, 0.0, 1.0);
    let tile_span = terrain_tile.procedural_grid.w;
    if spacing > 0.0 && grid_opacity > 0.0 && tile_span > 0.0 {
        let cells = tile_span / spacing;
        let lod_fade = 1.0 - smoothstep(0.18, 0.45, max(uv_fw.x, uv_fw.y) * cells);
        let g = in.tex_coords * cells;
        let gf = max(uv_fw * cells, vec2<f32>(1e-5));
        let dist = abs(fract(g - 0.5) - 0.5) / gf;
        let line = (1.0 - clamp(min(dist.x, dist.y), 0.0, 1.0)) * grid_opacity * lod_fade;
        rgb = mix(rgb, vec3<f32>(0.88, 0.92, 0.94), line);
    }
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv_fw = fwidth(in.tex_coords);
    let up_normal = normalize(in.surface_normal);
    let light = normalize(vec3<f32>(-0.5, 0.5, 1.0));
    let drape_uv = (terrain_tile.drape_matrix * vec4<f32>(in.tex_coords, 0.0, 1.0)).xy;
    let sampled_drape = textureSample(drape_texture, drape_sampler, drape_uv);
    var lit: vec4<f32>;
    if terrain_tile.procedural_relief.x > 0.5 {
        lit = select(
            procedural_surface(in, up_normal, light, uv_fw),
            terrain_tile.surface_color,
            in.polar_cap > 0.0,
        );
    } else {
        let relief = 1.0 + terrain_tile.relief_strength * (dot(up_normal, light) - light.z);
        let draped = select(sampled_drape, terrain_tile.surface_color,
            terrain_tile.fog_opacity.z > 0.5);
        let polar_color = unpack_rgba8(select(
            terrain_tile.edge_index.z,
            terrain_tile.edge_index.y,
            in.tex_coords.y < 0.5,
        ));
        // No tile holds the ground beyond the last row of tiles, so the cap closing the globe takes
        // the pole's flat vector/background fill, unlit, instead of stretching the drape's edge.
        lit = select(vec4<f32>(draped.rgb * relief, draped.a), polar_color,
            in.polar_cap > 0.0);
    }
    let surface = vec4<f32>(daylight(lit.rgb, in.globe_normal), lit.a);
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
