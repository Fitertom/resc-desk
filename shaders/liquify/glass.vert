#version 450
#extension GL_GOOGLE_include_directive : require
// Quad around the glass shape plus the shadow margin (triangle strip, 4 vertices).
#include "common.glsl"

void main() {
    vec2 corner = vec2(float(gl_VertexIndex & 1), float((gl_VertexIndex >> 1) & 1));
    vec2 m = vec2(GLASS_MARGIN * pc.scale);
    vec2 lo = pc.center - pc.halfSize - m;
    vec2 hi = pc.center + pc.halfSize + m + vec2(0.0, SHADOW_DROP * pc.scale);
    vec2 px = mix(lo, hi, corner);
    gl_Position = vec4(px / pc.screen * 2.0 - 1.0, 0.0, 1.0);
}
