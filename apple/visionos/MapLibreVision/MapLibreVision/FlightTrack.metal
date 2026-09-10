#include <metal_stdlib>
using namespace metal;

struct FlightVertex { float4 position; float4 color; };
struct FlightFragment { float4 position [[position]]; float4 color; };

vertex FlightFragment flightTrackVertex(uint id [[vertex_id]], constant FlightVertex *vertices [[buffer(0)]]) {
    return {vertices[id].position, vertices[id].color};
}

fragment float4 flightTrackFragment(FlightFragment in [[stage_in]]) {
    return in.color;
}

struct HUDVertex { float4 position; float2 uv; };
struct HUDFragment { float4 position [[position]]; float2 uv; };
vertex HUDFragment flightHUDVertex(uint id [[vertex_id]], constant HUDVertex* vertices [[buffer(0)]]) {
    return {vertices[id].position, vertices[id].uv};
}
fragment float4 flightHUDFragment(HUDFragment in [[stage_in]], texture2d<float> texture [[texture(0)]]) {
    constexpr sampler linearSampler(filter::linear, address::clamp_to_edge);
    const float4 color = texture.sample(linearSampler, in.uv);
    if (color.a < 0.001) discard_fragment();
    return color;
}

struct SymbolVertex { float4 position; float4 color; float edge; };
struct SymbolFragment { float4 position [[position]]; float4 color; float edge; };
vertex SymbolFragment flightSymbolVertex(uint id [[vertex_id]], constant SymbolVertex* vertices [[buffer(0)]]) {
    return {vertices[id].position, vertices[id].color, vertices[id].edge};
}
fragment float4 flightSymbolFragment(SymbolFragment in [[stage_in]]) {
    const float alpha = in.color.a * (1 - smoothstep(0.65, 1.0, abs(in.edge)));
    if (alpha < 0.01) discard_fragment();
    return float4(in.color.rgb * alpha, alpha);
}
