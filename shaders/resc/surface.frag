#version 450
#extension GL_GOOGLE_include_directive : require
#include "squircle.glsl"
// Premultiplied output (blend ONE, ONE_MINUS_SRC_ALPHA). A window's surfaces are clipped to
// its rounded corners (radius > 0). XRGB textures read alpha 1 through
// their image view's swizzle. 1:1 rects land on texel centres: the copy is exact; scaled ones
// are filtered bilinearly, as the CPU path does.

layout(set = 0, binding = 0) uniform sampler2D uTex;

layout(push_constant) uniform PC {
    vec2 screen;
    vec4 dst;
    vec4 uv;
    vec4 color;
    float mode;
    float alpha;
    vec4 clip;    // rounded clip on the output (x, y, w, h): a window's corners
    float radius;  // its top corners' radius; 0 (and radiusB 0): no clip
    float radiusB; // its bottom corners' radius
} pc;

layout(location = 0) in vec2 vUv;
layout(location = 0) out vec4 outColor;

void main() {
    vec4 c = pc.mode > 0.5 ? pc.color : texture(uTex, vUv);
    if (pc.radius > 0.0 || pc.radiusB > 0.0) {
        // Rounded window corners: coverage of the clip's rounded rect at this pixel (its top
        // and bottom corners each their own: a window's content in its drop, winanim.rs).
        vec2 hb = pc.clip.zw * 0.5;
        vec2 mid = pc.clip.xy + hb;
        float r = min(gl_FragCoord.y < mid.y ? pc.radius : pc.radiusB, min(hb.x, hb.y));
        // The continuous corners of the glass under it (liquify/squircle.glsl).
        float sd = sdSquircle(gl_FragCoord.xy - mid, hb, r);
        c *= clamp(0.5 - sd, 0.0, 1.0);
    }
    outColor = c * pc.alpha;
}
