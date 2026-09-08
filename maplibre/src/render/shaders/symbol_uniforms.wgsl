struct SymbolUniforms {
    text_color: vec4<f32>,
    halo_color: vec4<f32>,
    icon_color: vec4<f32>,
    icon_halo_color: vec4<f32>,
    text: vec4<f32>,
    icon: vec4<f32>,
    text_layout: vec4<f32>,
    icon_layout: vec4<f32>,
    atlas: vec4<f32>,
    placement: vec4<f32>,
};
@group(1) @binding(2) var<uniform> symbol: SymbolUniforms;
