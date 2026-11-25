#version 450
#extension GL_GOOGLE_include_directive : require
// Plain background for the whole screen. The glass pass then redraws only its own quad.
#define NEEDS_BACKGROUND
#include "common.glsl"

layout(location = 0) out vec4 outColor;

void main() {
    vec2 px = gl_FragCoord.xy;
    if (pc.region.w > 0.5) px = px * pc.region.z + pc.region.xy; // backdrop pass: its pixels -> screen pixels
    outColor = vec4(sampleBg(px / pc.screen), 1.0);
}
