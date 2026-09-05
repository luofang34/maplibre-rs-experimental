struct VertexOutput {
    @location(0) tex_coords: vec3<f32>,
    @location(1) horizon_distance: f32,
    @location(2) mercator_y: f32,
    @location(3) zoom: f32,
    @builtin(position) position: vec4<f32>,
};

@group(1) @binding(0)
var t_dem: texture_2d<f32>;
@group(1) @binding(1)
var s_dem: sampler;

struct HillshadeUniforms {
    unpack: vec4<f32>,
    accent: vec4<f32>,
    shadows: array<vec4<f32>, 4>,
    highlights: array<vec4<f32>, 4>,
    altitudes: vec4<f32>,
    azimuths: vec4<f32>,
    exaggeration: f32,
    method: u32,
    light_count: u32,
    padding: u32,
};

@group(2) @binding(0)
var<uniform> hillshade: HillshadeUniforms;

const PI: f32 = 3.141592653589793;
const STANDARD: u32 = 0u;
const COMBINED: u32 = 1u;
const IGOR: u32 = 2u;
const MULTIDIRECTIONAL: u32 = 3u;
const BASIC: u32 = 4u;

fn elevation_at(texel: vec2<i32>, dim: vec2<i32>) -> f32 {
    let clamped = clamp(texel, vec2<i32>(0, 0), dim - vec2<i32>(1, 1));
    let data = textureLoad(t_dem, clamped, 0) * 255.0;
    return dot(vec4<f32>(data.rgb, -1.0), hillshade.unpack);
}

fn get_aspect(deriv: vec2<f32>) -> f32 {
    if deriv.x != 0.0 {
        return atan2(deriv.y, -deriv.x);
    }
    return PI / 2.0 * select(-1.0, 1.0, deriv.y > 0.0);
}

fn igor_hillshade(deriv_in: vec2<f32>) -> vec4<f32> {
    let deriv = deriv_in * hillshade.exaggeration * 2.0;
    let aspect = get_aspect(deriv);
    let azimuth = hillshade.azimuths[0] + PI;
    let slope_strength = atan(length(deriv)) * 2.0 / PI;
    let aspect_strength = 1.0 - abs(((aspect + azimuth) / PI + 0.5) % 2.0 - 1.0);
    let shadow_strength = slope_strength * aspect_strength;
    let highlight_strength = slope_strength * (1.0 - aspect_strength);
    return hillshade.shadows[0] * shadow_strength + hillshade.highlights[0] * highlight_strength;
}

fn standard_hillshade(deriv: vec2<f32>) -> vec4<f32> {
    let azimuth = hillshade.azimuths[0] + PI;
    let slope = atan(0.625 * length(deriv));
    let aspect = get_aspect(deriv);
    let intensity = hillshade.exaggeration;
    let base = 1.875 - intensity * 1.75;
    let max_value = 0.5 * PI;
    var scaled_slope = slope;
    if intensity != 0.5 {
        scaled_slope = ((pow(base, slope) - 1.0) / (pow(base, max_value) - 1.0)) * max_value;
    }
    let accent = cos(scaled_slope);
    let accent_color = (1.0 - accent) * hillshade.accent * clamp(intensity * 2.0, 0.0, 1.0);
    let shade = abs(((aspect + azimuth) / PI + 0.5) % 2.0 - 1.0);
    let shade_color = mix(hillshade.shadows[0], hillshade.highlights[0], shade)
        * sin(scaled_slope) * clamp(intensity * 2.0, 0.0, 1.0);
    return accent_color * (1.0 - shade_color.a) + shade_color;
}

fn directional_shade(deriv: vec2<f32>, cos_az: f32, sin_az: f32, altitude: f32) -> f32 {
    let cos_alt = cos(altitude);
    let sin_alt = sin(altitude);
    let cang = (sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt))
        / sqrt(1.0 + dot(deriv, deriv));
    return clamp(cang, 0.0, 1.0);
}

fn shade_color(shade: f32, light: u32, weight: f32) -> vec4<f32> {
    if shade > 0.5 {
        return hillshade.highlights[light] * (2.0 * shade - 1.0) * weight;
    }
    return hillshade.shadows[light] * (1.0 - 2.0 * shade) * weight;
}

fn basic_hillshade(deriv_in: vec2<f32>) -> vec4<f32> {
    let deriv = deriv_in * hillshade.exaggeration * 2.0;
    let azimuth = hillshade.azimuths[0] + PI;
    let shade = directional_shade(deriv, cos(azimuth), sin(azimuth), hillshade.altitudes[0]);
    return shade_color(shade, 0u, 1.0);
}

fn multidirectional_hillshade(deriv_in: vec2<f32>) -> vec4<f32> {
    let deriv = deriv_in * hillshade.exaggeration * 2.0;
    let count = max(hillshade.light_count, 1u);
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var light = 0u; light < count; light = light + 1u) {
        let shade = directional_shade(
            deriv,
            -cos(hillshade.azimuths[light]),
            -sin(hillshade.azimuths[light]),
            hillshade.altitudes[light],
        );
        color = color + shade_color(shade, light, 1.0 / f32(count));
    }
    return color;
}

fn combined_hillshade(deriv_in: vec2<f32>) -> vec4<f32> {
    let deriv = deriv_in * hillshade.exaggeration * 2.0;
    let azimuth = hillshade.azimuths[0] + PI;
    let cos_az = cos(azimuth);
    let sin_az = sin(azimuth);
    let cos_alt = cos(hillshade.altitudes[0]);
    let sin_alt = sin(hillshade.altitudes[0]);
    var cang = acos((sin_alt - (deriv.y * cos_az * cos_alt - deriv.x * sin_az * cos_alt))
        / sqrt(1.0 + dot(deriv, deriv)));
    cang = clamp(cang, 0.0, PI / 2.0);
    let shade = cang * atan(length(deriv)) * 4.0 / PI / PI;
    let highlight = (PI / 2.0 - cang) * atan(length(deriv)) * 4.0 / PI / PI;
    return hillshade.shadows[0] * shade + hillshade.highlights[0] * highlight;
}

@fragment
fn main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.horizon_distance < 0.0 {
        discard;
    }
    let dim = vec2<i32>(textureDimensions(t_dem));
    let uv = in.tex_coords.xy / in.tex_coords.z;
    let pos = vec2<i32>(floor(uv * vec2<f32>(dim)));
    let a = elevation_at(pos + vec2<i32>(-1, -1), dim);
    let b = elevation_at(pos + vec2<i32>(0, -1), dim);
    let c = elevation_at(pos + vec2<i32>(1, -1), dim);
    let d = elevation_at(pos + vec2<i32>(-1, 0), dim);
    let f = elevation_at(pos + vec2<i32>(1, 0), dim);
    let g = elevation_at(pos + vec2<i32>(-1, 1), dim);
    let h = elevation_at(pos + vec2<i32>(0, 1), dim);
    let i = elevation_at(pos + vec2<i32>(1, 1), dim);

    // Slopes are divided by eight times the pixel size in metres; the exaggeration keeps the
    // shading visible at low zooms, as the GL JS prepare pass does.
    let tile_size = f32(dim.x);
    let zoom = in.zoom;
    let exaggeration_factor = select(select(0.3, 0.35, zoom < 4.5), 0.4, zoom < 2.0);
    let exaggeration = select(0.0, (zoom - 15.0) * exaggeration_factor, zoom < 15.0);
    var deriv = vec2<f32>(
        (c + f + f + i) - (a + d + d + g),
        (g + h + h + i) - (a + b + b + c),
    ) * tile_size / pow(2.0, exaggeration + (28.2562 - zoom));
    deriv = clamp(deriv, vec2<f32>(-4.0, -4.0), vec2<f32>(4.0, 4.0));

    // The Mercator projection stretches distances with latitude.
    let latitude = atan(sinh(PI * (1.0 - 2.0 * in.mercator_y)));
    deriv = deriv / cos(latitude);

    switch hillshade.method {
        case BASIC: {
            return basic_hillshade(deriv);
        }
        case COMBINED: {
            return combined_hillshade(deriv);
        }
        case IGOR: {
            return igor_hillshade(deriv);
        }
        case MULTIDIRECTIONAL: {
            return multidirectional_hillshade(deriv);
        }
        default: {
            return standard_hillshade(deriv);
        }
    }
}
