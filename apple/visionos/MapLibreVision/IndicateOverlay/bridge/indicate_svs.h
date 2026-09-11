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

typedef struct { float a[3]; float b[3]; } AngularStroke;
typedef struct { uint32_t length; AngularStroke strokes[1024]; } AngularScene;
_Static_assert(sizeof(AngularScene) == 24580, "Angular scene ABI");
AngularScene indicate_svs_directions(ReplayTelemetry input);
typedef struct { uint32_t kind; float right[3]; float up[3]; float forward[3]; } ViewReference;
_Static_assert(sizeof(ViewReference) == 40, "View reference ABI");
ViewReference indicate_svs_reference(ReplayTelemetry input);
OverlayScene indicate_svs_render(ReplayTelemetry input);
OverlayScene indicate_svs_glance(ReplayTelemetry input);
uint64_t indicate_svs_glyph(uint32_t scalar);
#endif
