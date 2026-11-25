#version 450
#extension GL_GOOGLE_include_directive : require
// The soft shadow under a glass panel: the first lines of liquify/glass.frag, unchanged, with
// glass.vert's quad. The glass itself is liquify/ui.frag kind 2, drawn over it right after
// (as the CPU path does in glass.rs).

#include "common.glsl"

layout(location = 0) out vec4 outColor;

void main() {
    vec2 px = gl_FragCoord.xy;               // Vulkan: origin top-left

    vec2 p = px - pc.center;
    float sd = sdRoundedRect(p, pc.halfSize, pc.radius);

    // Soft shadow under the glass, slightly shifted down.
    float ss = max(sdRoundedRect(p - vec2(0.0, SHADOW_DROP * pc.scale), pc.halfSize, pc.radius), 0.0);
    float shadowA = ss < (GLASS_MARGIN - 2.0) * pc.scale ? exp(-ss * ss / (800.0 * pc.scale * pc.scale)) : 0.0;
    // Premultiplied output over the frame (blend ONE, ONE_MINUS_SRC_ALPHA): outside the shape only the
    // shadow darkens what is already there.
    float shadowK = pc.shadow * 0.6 * shadowA;
    if (sd > 0.0) {
        outColor = vec4(0.0, 0.0, 0.0, shadowK);
        return;
    }
    discard;
}
