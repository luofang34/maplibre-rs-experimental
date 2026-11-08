struct VertexOutput {
    @location(0) @interpolate(flat) sky_color: vec4<f32>,
    @location(1) @interpolate(flat) horizon_color: vec4<f32>,
    @location(2) @interpolate(flat) horizon: vec4<f32>,
    @location(3) @interpolate(flat) blend: vec4<f32>,
    @location(4) view_direction: vec3<f32>,
    @location(5) @interpolate(flat) up_and_horizon: vec4<f32>,
    @location(6) @interpolate(flat) globe_params: vec4<f32>,
    @builtin(position) position: vec4<f32>,
};

const ATMOSPHERE_HEIGHT_TO_GLOBE_RADIUS: f32 = 0.015696123057604772; // 100000.0 / 6371008.8

// The globe sky: horizon colour at the eye's sea-level horizon, easing into the sky colour
// over the blend angle above it. Only the ray's elevation matters, never its heading.
fn globe_sky(in: VertexOutput) -> vec4<f32> {
    let ray = normalize(in.view_direction);
    let up = normalize(in.up_and_horizon.xyz);
    let elevation = asin(clamp(dot(ray, up), -1.0, 1.0));
    let above_horizon = max(elevation - in.up_and_horizon.w, 0.0);
    let t = clamp(above_horizon / max(in.blend.x, 1e-4), 0.0, 1.0);
    let toward_horizon = 1.0 - t;
    let color = mix(in.sky_color, in.horizon_color, toward_horizon * toward_horizon);
    return mix(color, vec4<f32>(0.0, 0.0, 0.0, 0.0), in.blend.y);
}

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.blend.z < 0.0 {
        let output = globe_sky(in);
        if output.a <= 0.0 { discard; }
        return output;
    }
    // GL JS measures from the bottom of the screen.
    let x = in.position.x;
    let y = in.blend.z - in.position.y;
    let signed_distance = (y - in.horizon.y) * in.horizon.w + (x - in.horizon.x) * in.horizon.z;
    // An external eye can see sky below the sea-level horizon through a valley.
    // Terrain depth hides this distant background wherever ground actually exists.
    var distance = select(signed_distance, max(signed_distance, 0.0), in.blend.w != 0.0);
    var opacity = 1.0;
    let sky_blend = in.blend.y;
    let globe_radius = in.globe_params.z;
    if sky_blend > 0.0 && globe_radius > 0.0 {
        let ray = normalize(in.view_direction);
        let globe_position = in.up_and_horizon.xyz;
        let globe_distance = length(globe_position);
        let angle_to_globe_center =
            acos(clamp(dot(ray, globe_position) / globe_distance, -1.0, 1.0));
        let horizon_angle = asin(min(globe_radius / globe_distance, 1.0));
        let globe_distance_px =
            ((angle_to_globe_center - horizon_angle) * in.globe_params.x + 1.0)
            * in.globe_params.y;
        distance = mix(distance, globe_distance_px, sky_blend);

        let closest_approach = max(dot(ray, globe_position), 0.0);
        let lowest_altitude = sqrt(max(
            globe_distance * globe_distance - closest_approach * closest_approach,
            0.0,
        )) - globe_radius;
        let atmosphere_height = globe_radius * ATMOSPHERE_HEIGHT_TO_GLOBE_RADIUS;
        let inside_atmosphere = 1.0 - smoothstep(
            atmosphere_height,
            10.0 * atmosphere_height,
            globe_distance - globe_radius,
        );
        let through_atmosphere = 1.0 - smoothstep(0.0, atmosphere_height, lowest_altitude);
        opacity = inside_atmosphere * through_atmosphere * (1.0 - in.globe_params.w);
    }
    if distance <= 0.0 && in.blend.w == 0.0 {
        discard;
    }
    var color = in.sky_color;
    if distance < in.blend.x {
        color = mix(in.sky_color, in.horizon_color, pow(1.0 - distance / in.blend.x, 2.0));
    }
    let output = color * mix(1.0, opacity, sky_blend);
    if output.a <= 0.0 { discard; }
    return output;
}
