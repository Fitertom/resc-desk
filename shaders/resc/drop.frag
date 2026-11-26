#version 450
#extension GL_GOOGLE_include_directive : require
// A drop of liquid glass (winanim.rs): a window or the preview card on its way out of or into
// its taskbar button, smooth-unioned (SDF smin, radius k) with the taskbar's hover drop under
// the button, so a bridge of glass swells between them while it lets go or lands. The glass is
// Liquify's card as liquify/ui.frag kind 2 draws it (refraction along the bezel, the frosted
// backdrop with saturate(1.6) brightness(1.02), tint, inner shadow, specular rim, inset
// highlight, hairline border; refraction model from archisvaze/liquid-glass, MIT, see
// liquify/glass.frag), over the union's distance; clear glass (settings) as liquify/glass.frag
// draws it. Plus the shape's soft shadow and a window frame's rim light (shell.rs
// `draw_shadow`, `draw_deco`). Each shape keeps its own look (tint, shadow, rim, bezel),
// blended across the bridge. Premultiplied output (blend ONE, ONE_MINUS_SRC_ALPHA).

#include "squircle.glsl"
#include "bend.glsl"

layout(set = 0, binding = 1) uniform sampler2D uWall; // what lies under the glass (the frame so far)

layout(push_constant) uniform PC {
    vec2 screen;
    float frost; // sigma px over the sharp copy (its mips); 0: the copy is blurred already
    float clear; // 1: clear glass
} pc;

layout(location = 0) in vec2 vPx;
layout(location = 1) flat in vec4 vA;     // the drop: x0, y0, x1, y1
layout(location = 2) flat in vec4 vB;     // the hover drop (empty: none)
layout(location = 3) flat in vec4 vGeo;   // corners of A, of B; k; px per CSS px
layout(location = 4) flat in vec4 vTintA; // premultiplied
layout(location = 5) flat in vec4 vTintB;
layout(location = 6) flat in vec4 vShA;   // shadow: strength, sigma, drop, reach
layout(location = 7) flat in vec4 vShB;
layout(location = 8) flat in vec4 vLookA; // rim light at the top, at the bottom; inset highlight; border
layout(location = 9) flat in vec4 vLookB;
layout(location = 10) flat in vec4 vBez;  // bezel of A, of B; shadow falloff of A, of B (1: squared to its reach)
layout(location = 11) flat in vec4 vMisc; // glass thickness, border width

layout(location = 0) out vec4 outColor;

bool hasB() {
    return vB.z - vB.x >= 1.0 && vB.w - vB.y >= 1.0;
}

float sdRect(vec2 p, vec4 r, float rad) {
    vec2 hb = max((r.zw - r.xy) * 0.5, vec2(0.0));
    return sdSquircle(p - (r.xy + hb), hb, rad);
}

float sdA(vec2 p) {
    return sdRect(p, vA, vGeo.x);
}

float sdB(vec2 p) {
    return hasB() ? sdRect(p, vB, vGeo.y) : 1e6;
}

// Smooth union (polynomial smin): the bridge's radius k.
float smin(float a, float b, float k) {
    if (k <= 0.0) return min(a, b);
    float h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

float shape(vec2 p) {
    return smin(sdA(p), sdB(p), vGeo.z);
}

// ui.frag `wallFrost`: the backdrop ("cover" fit of a screen-sized copy: as is), blurred, then
// saturate(1.6) brightness(1.02). Clear glass: the backdrop as it is.
vec3 frost(vec2 px) {
    vec2 uv = px / pc.screen;
    if (pc.clear > 0.5) return textureLod(uWall, uv, 0.0).rgb;
    vec3 c;
    if (pc.frost > 0.0) {
        // ui.frag's fine kernel: a 4 x 4 gaussian grid on the level a step finer than sigma.
        vec2 tex = vec2(textureSize(uWall, 0));
        float s = pc.frost * max(tex.x / pc.screen.x, tex.y / pc.screen.y);
        vec2 t = pc.frost / pc.screen;
        float lod = max(log2(s) - 1.0, 0.0);
        c = vec3(0.0);
        float wsum = 0.0;
        for (int j = 0; j < 4; j++) {
            for (int i = 0; i < 4; i++) {
                vec2 d = (vec2(float(i), float(j)) - 1.5) * 0.85;
                float w = exp(-0.5 * dot(d, d));
                c += w * textureLod(uWall, uv + d * t, lod).rgb;
                wsum += w;
            }
        }
        c /= wsum;
    } else {
        vec2 o = vec2(0.75 * vGeo.w) / pc.screen;
        c = 0.25 * (texture(uWall, uv + vec2(-o.x, -o.y)).rgb + texture(uWall, uv + vec2(o.x, -o.y)).rgb
                  + texture(uWall, uv + vec2(-o.x, o.y)).rgb + texture(uWall, uv + vec2(o.x, o.y)).rgb);
    }
    float lum = dot(c, vec3(0.2126, 0.7152, 0.0722));
    return clamp((lum + (c - lum) * 1.6) * 1.02, 0.0, 1.0);
}

vec4 over(vec4 top, vec4 bottom) {
    return top + bottom * (1.0 - top.a);
}

// Bezel height profile: 0 at the edge, 1 where the flat top starts (ui.frag, glass.frag).
float surfaceHeight(float t) {
    float s = 1.0 - t;
    return pow(1.0 - s * s * s * s, 0.25);
}

// ui.frag: the bezel asked for, at most the corners' radius (or half the bezel when that is
// more) and half the shape's short side.
float bezelOf(vec4 r, float rad, float want) {
    vec2 hb = max((r.zw - r.xy) * 0.5, vec2(0.0));
    return max(1.0, min(want, min(max(rad, want * 0.5), min(hb.x, hb.y)) - 1.0));
}

const float IOR = 3.0;
const float SPEC = 0.55;

void main() {
    vec2 p = vPx;
    float s = vGeo.w;
    float k = vGeo.z;
    float a = sdA(p);
    float b = sdB(p);
    float sd = smin(a, b, k);
    // How much of this pixel's look is the drop's (A) rather than the hover drop's (B).
    float wa = hasB() ? clamp(0.5 + 0.5 * (b - a) / max(k, 2.0 * s), 0.0, 1.0) : 1.0;

    // The shape's soft shadow, a little lower.
    vec4 sh = mix(vShB, vShA, wa);
    float ss = max(shape(p - vec2(0.0, sh.z)), 0.0);
    float shadow = 0.0;
    if (ss < sh.w && sh.x > 0.0) {
        float f = 1.0 - ss / sh.w;
        shadow = sh.x * exp(-ss * ss / (2.0 * sh.y * sh.y)) * mix(1.0, f * f, mix(vBez.w, vBez.z, wa));
    }
    float cov = clamp(0.5 - sd, 0.0, 1.0);
    if (cov <= 0.0) {
        if (shadow <= 0.0) discard;
        outColor = vec4(0.0, 0.0, 0.0, shadow);
        return;
    }

    bool clearGlass = pc.clear > 0.5;
    vec4 tint = clearGlass ? vec4(0.0) : mix(vTintB, vTintA, wa);
    vec4 look = mix(vLookB, vLookA, wa);
    float bzA = bezelOf(vA, vGeo.x, vBez.x);
    float bezel = hasB() ? mix(bezelOf(vB, vGeo.y, vBez.y), bzA, wa) : bzA;
    float thick = vMisc.x;
    float d = -sd;
    vec4 col;
    if (d >= bezel) {
        col = over(tint, vec4(frost(p), 1.0));
    } else {
        float t = clamp(d / bezel, 0.0, 1.0);
        float h = surfaceHeight(t);
        float dh = (surfaceHeight(min(t + 0.001, 1.0)) - h) / 0.001;
        float slope = atan(dh * (thick / bezel));
        float thetaR = asin(clamp(sin(slope) / IOR, -1.0, 1.0));
        float displacement = bendDisplacement(d, bezel, thick, IOR);
        vec2 grad = normalize(vec2(shape(p + vec2(0.5, 0.0)) - sd, shape(p + vec2(0.0, 0.5)) - sd) + 1e-6);
        vec4 under = vec4(frost(p - grad * displacement), 1.0);
        float innerShadow = 1.0 - smoothstep(0.0, bezel * 0.6, d);
        under.rgb *= mix(1.0, 0.7, innerShadow * 0.3);
        col = over(tint, under);
        vec2 lightDir = normalize(vec2(0.5, -0.7));
        float facing = abs(dot(grad, lightDir));
        float line = 1.0 - smoothstep(0.0, 1.6 * s, d);
        float glow = 1.0 - smoothstep(0.0, bezel * 0.25, d);
        float spec = clamp(SPEC * (line * mix(0.35, 1.0, facing * facing) + 0.35 * pow(facing * glow, 2.0)), 0.0, 1.0);
        col.rgb = 1.0 - (1.0 - col.rgb) * (1.0 - spec);
    }
    if (!clearGlass) {
        // CSS `inset 1px 1px 0 white(a)`: the part of the shape not covered by itself moved right-down.
        if (look.z > 0.0) {
            float band = 1.0 - clamp(0.5 - shape(p - vec2(s)), 0.0, 1.0);
            col = over(vec4(look.z * band), col);
        }
        // Hairline border.
        float bw = vMisc.y;
        if (look.w > 0.0 && bw > 0.0) {
            float inner = clamp(0.5 - (sd + bw), 0.0, 1.0);
            col = over(vec4(look.w) * (1.0 - inner), col);
        }
    }
    // A window frame's rim light: a hairline along the edge, brighter at the top.
    if (look.x + look.y > 0.0) {
        float ty = clamp((p.y - vA.y) / max(vA.w - vA.y, 1.0), 0.0, 1.0);
        float ra = mix(look.x, look.y, sqrt(ty));
        float band = clamp(sd + s + 0.5, 0.0, 1.0) * clamp(0.5 - sd, 0.0, 1.0);
        col = over(vec4(ra * band), col);
    }
    // Only the rim (Alt+Tab's panel, vMisc.z): the inside is left out, the glass fading away
    // 2.5 to 6 px in from the edge.
    if (vMisc.z > 0.5) {
        col *= 1.0 - smoothstep(2.5 * s, 6.0 * s, d);
    }
    // Clear glass fades in over its outer 1.5 px (glass.frag).
    if (clearGlass) cov = smoothstep(0.0, 1.5 * s, d);
    outColor = col * cov + vec4(0.0, 0.0, 0.0, shadow) * (1.0 - cov);
}
