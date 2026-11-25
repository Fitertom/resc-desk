#version 450
// A drop of glass (drop.frag): its quad, the shapes' box grown by the shadow's reach,
// physical pixels. Triangle strip, 4 vertices, one instance of 12 vec4 (gpu/mod.rs
// `drop_inst`); everything but the quad goes to the fragments as it is.

layout(location = 0) in vec4 iQuad;  // x0, y0, x1, y1
layout(location = 1) in vec4 iA;
layout(location = 2) in vec4 iB;
layout(location = 3) in vec4 iGeo;
layout(location = 4) in vec4 iTintA;
layout(location = 5) in vec4 iTintB;
layout(location = 6) in vec4 iShA;
layout(location = 7) in vec4 iShB;
layout(location = 8) in vec4 iLookA;
layout(location = 9) in vec4 iLookB;
layout(location = 10) in vec4 iBez;
layout(location = 11) in vec4 iMisc;

layout(push_constant) uniform PC {
    vec2 screen;
    float frost;
    float clear;
} pc;

layout(location = 0) out vec2 vPx;
layout(location = 1) flat out vec4 vA;
layout(location = 2) flat out vec4 vB;
layout(location = 3) flat out vec4 vGeo;
layout(location = 4) flat out vec4 vTintA;
layout(location = 5) flat out vec4 vTintB;
layout(location = 6) flat out vec4 vShA;
layout(location = 7) flat out vec4 vShB;
layout(location = 8) flat out vec4 vLookA;
layout(location = 9) flat out vec4 vLookB;
layout(location = 10) flat out vec4 vBez;
layout(location = 11) flat out vec4 vMisc;

void main() {
    vec2 corner = vec2(float(gl_VertexIndex & 1), float((gl_VertexIndex >> 1) & 1));
    vec2 px = mix(iQuad.xy, iQuad.zw, corner);
    vPx = px;
    vA = iA;
    vB = iB;
    vGeo = iGeo;
    vTintA = iTintA;
    vTintB = iTintB;
    vShA = iShA;
    vShB = iShB;
    vLookA = iLookA;
    vLookB = iLookB;
    vBez = iBez;
    vMisc = iMisc;
    gl_Position = vec4(px / pc.screen * 2.0 - 1.0, 0.0, 1.0);
}
