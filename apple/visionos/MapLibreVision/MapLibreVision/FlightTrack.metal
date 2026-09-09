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
