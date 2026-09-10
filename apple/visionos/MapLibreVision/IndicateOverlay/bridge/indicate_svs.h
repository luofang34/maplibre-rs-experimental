#ifndef INDICATE_SVS_H
#define INDICATE_SVS_H
#include <stdint.h>

typedef struct {
    uint32_t present;
    float age_ms;
    float ground_speed;
    float track;
    float altitude_msl;
    float vertical_speed;
    float ias;
    float roll;
    float pitch;
    float heading;
} ReplayTelemetry;

typedef struct {
    uint32_t length;
    uint8_t bytes[8192];
} OverlayScene;

_Static_assert(sizeof(ReplayTelemetry) == 40, "Replay telemetry ABI");
_Static_assert(sizeof(OverlayScene) == 8196, "Overlay scene ABI");

OverlayScene indicate_svs_render(ReplayTelemetry input);
uint64_t indicate_svs_glyph(uint32_t scalar);
#endif
