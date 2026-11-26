#version 450
#extension GL_GOOGLE_include_directive : require
// Liquid glass over a background texture. Runs only inside the quad from glass.vert.
//
// Refraction model adapted from archisvaze/liquid-glass (webgl.html):
// MIT License, Copyright (c) 2026 Archis Vaze.
// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and
// to permit persons to whom the Software is furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all copies or substantial portions of
// the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO
// THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
// CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.
//
// Additions here: optional dispersion, saturation, adaptive tint, crisp screen-blended rim, DPI scale.
// All sizes are in physical pixels.

#define NEEDS_BACKGROUND
#include "common.glsl"
#include "bend.glsl"

layout(location = 0) out vec4 outColor;

// Bezel height profile: 0 at the edge, 1 where the flat top starts.
float surfaceHeight(float t) {
    float s = 1.0 - t;
    return pow(1.0 - s * s * s * s, 0.25);
}

const vec2 OFFS[16] = vec2[16](
    vec2(-0.94201, -0.39906), vec2( 0.94558, -0.76890), vec2(-0.09418, -0.92938), vec2( 0.34495,  0.29387),
    vec2(-0.91588, -0.45771), vec2(-0.81544,  0.48568), vec2(-0.38277, -0.56071), vec2(-0.12675,  0.84686),
    vec2( 0.89642,  0.41254), vec2( 0.18150, -0.30020), vec2(-0.01445, -0.16001), vec2( 0.59614,  0.71118),
    vec2( 0.49742, -0.47280), vec2( 0.80685,  0.04588), vec2(-0.32490, -0.03965), vec2(-0.60975,  0.06566)
);

vec3 sampleBlurred(vec2 px, float r) {
    vec2 inv = 1.0 / pc.screen;
    if (r < 0.5) return sampleBg(px * inv);
    vec3 sum = vec3(0.0);
    // The web's liquid glass blurs only 1.5 px: 8 taps are plenty.
    const int n = 8;
    for (int i = 0; i < n; i++) sum += sampleBg((px + OFFS[i] * r) * inv);
    return sum / float(n);
}

void main() {
    vec2 px = gl_FragCoord.xy;               // Vulkan: origin top-left

    vec2 p = px - pc.center;
    float sd = sdRoundedRect(p, pc.halfSize, pc.radius);

    // Soft shadow under the glass, slightly shifted down.
    // Cut to exactly zero just inside the glass quad (there it is < 0.0002), so the quad path is
    // bit-identical to drawing the glass shader over the whole screen.
    float ss = max(sdRoundedRect(p - vec2(0.0, SHADOW_DROP * pc.scale), pc.halfSize, pc.radius), 0.0);
    float shadowA = ss < (GLASS_MARGIN - 2.0) * pc.scale ? exp(-ss * ss / (800.0 * pc.scale * pc.scale)) : 0.0;
    // Premultiplied output over the frame (blend ONE, ONE_MINUS_SRC_ALPHA): outside the shape only the
    // shadow darkens what is already there.
    float shadowK = pc.shadow * 0.6 * shadowA;
    if (sd > 0.0) {
        outColor = vec4(0.0, 0.0, 0.0, shadowK);
        return;
    }

    float d = -sd;
    float bezel = max(1.0, min(pc.bezel, min(pc.radius, min(pc.halfSize.x, pc.halfSize.y)) - 1.0));
    if (d >= bezel) {
        // Flat top: no refraction, rim or inner shadow; only the frosted backdrop, saturation and tint.
        vec3 c = sampleBlurred(px, pc.blur);
        float l0 = dot(c, vec3(0.299, 0.587, 0.114));
        c = max(mix(vec3(l0), c, pc.sat), 0.0);
        float l1 = dot(c, vec3(0.299, 0.587, 0.114));
        c = pc.tint >= 0.0 ? mix(c, vec3(1.0), pc.tint * smoothstep(0.04, 0.5, l1)) : mix(c, vec3(0.0), -pc.tint);
        outColor = vec4(c, 1.0);
        return;
    }
    float t = clamp(d / bezel, 0.0, 1.0);

    float h = surfaceHeight(t);
    float dh = (surfaceHeight(min(t + 0.001, 1.0)) - h) / 0.001;
    float slope = atan(dh * (pc.thickness / bezel));
    float thetaR = asin(clamp(sin(slope) / pc.ior, -1.0, 1.0));
    float displacement = bendDisplacement(d, bezel, pc.thickness, pc.ior);

    vec2 grad = vec2(
        sdRoundedRect(p + vec2(0.5, 0.0), pc.halfSize, pc.radius) - sd,
        sdRoundedRect(p + vec2(0.0, 0.5), pc.halfSize, pc.radius) - sd);
    grad = normalize(grad + vec2(1e-6));
    vec2 offset = -grad * displacement;

    vec3 color;
    if (pc.disp > 0.001) {
        float k = pc.disp * 0.14;
        color.r = sampleBlurred(px + offset * (1.0 + k), pc.blur).r;
        color.g = sampleBlurred(px + offset, pc.blur).g;
        color.b = sampleBlurred(px + offset * (1.0 - k), pc.blur).b;
    } else {
        color = sampleBlurred(px + offset, pc.blur);
    }

    float luma = dot(color, vec3(0.299, 0.587, 0.114));
    color = max(mix(vec3(luma), color, pc.sat), 0.0);

    float innerShadow = 1.0 - smoothstep(0.0, bezel * 0.6, d);
    color *= mix(1.0, 0.7, innerShadow * 0.3);

    // Adaptive tint: a white wash only where the content is bright enough, so black stays black.
    float lum = dot(color, vec3(0.299, 0.587, 0.114));
    color = pc.tint >= 0.0
        ? mix(color, vec3(1.0), pc.tint * smoothstep(0.04, 0.5, lum))
        : mix(color, vec3(0.0), -pc.tint);

    // Specular: a crisp ~1.5px rim line (brighter on the light-facing sides) plus a faint narrow glow.
    // Screen blend, so it reads as a clean edge on black and never blows out on white.
    vec2 lightDir = normalize(vec2(0.5, -0.7));
    float facing = abs(dot(grad, lightDir));
    float line = 1.0 - smoothstep(0.0, 1.6 * pc.scale, d);
    float glow = 1.0 - smoothstep(0.0, bezel * 0.25, d);
    float spec = pc.spec * (line * mix(0.35, 1.0, facing * facing) + 0.35 * pow(facing * glow, 2.0));
    color = 1.0 - (1.0 - color) * (1.0 - clamp(spec, 0.0, 1.0));

    float alpha = smoothstep(0.0, 1.5 * pc.scale, d);
    outColor = vec4(color * alpha, alpha + (1.0 - alpha) * shadowK);
}
