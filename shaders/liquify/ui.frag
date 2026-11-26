#version 450
#extension GL_GOOGLE_include_directive : require
// UI instances. Output is premultiplied alpha (blend ONE, ONE_MINUS_SRC_ALPHA).
// Kinds: 0 shape, 1 atlas mask (glyph / icon), 2 liquid-glass shape over the wallpaper, 3 soft shadow,
// 4 rare shapes (vector quads, wallpaper panels).
//
// Kind 2 is the refraction model of glass.frag (archisvaze/liquid-glass, MIT, see there) over the wallpaper,
// with the web cards' filter: blur(1.5px) saturate(1.6) brightness(1.02) (the web's liquid-glass mode).

layout(set = 0, binding = 0) uniform sampler2D uAtlas;
layout(set = 0, binding = 1) uniform sampler2D uWall; // the wallpaper
layout(set = 0, binding = 2) uniform sampler2D uImg;  // pictures (covers), premultiplied RGBA
layout(set = 0, binding = 3) uniform sampler2D uVideo; // the playing video (a Spotify canvas), RGBA
layout(set = 0, binding = 4) uniform sampler2D uCover; // the player's cover, at its size on screen

// The web's canvas video filter: saturate(1.08) contrast(1.04) brightness(.88).
vec3 videoLook(vec3 c) {
    float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
    c = l + (c - l) * 1.08;
    c = (c - 0.5) * 1.04 + 0.5;
    return clamp(c * 0.88, 0.0, 1.0);
}

layout(constant_id = 0) const int KIND = 0;

layout(push_constant) uniform PC {
    vec2 screen;
    float bgAspect;
    float scrollY; // content offset (scrolling), physical px
    vec4 round;    // a clip with round corners over everything (x0, y0, x1, y1), screen px
    float roundR;  // its radius (0: none)
    float lens;    // its lens: rim width, px (0: none)
    float frost;   // glass over a sharp background (see-through): its blur, gaussian sigma px (0: a light frost)
    float frostQ;  // 1: the finer kernel; 2: no wallpaper under the glass (DWM draws glass under the window)
} pc;

layout(location = 0) in vec2 vPx;
layout(location = 1) in vec2 vUv;
layout(location = 2) flat in vec4 vRect;
layout(location = 3) flat in vec4 vC0;
layout(location = 4) flat in vec4 vC1;
layout(location = 5) flat in vec4 vGrad;
layout(location = 6) flat in vec4 vBorder;
layout(location = 7) flat in vec4 vRadii; // top-left, top-right, bottom-right, bottom-left
layout(location = 8) flat in vec4 vP;     // border width, kind, gradient type, shadow blur (glass: opacity)
layout(location = 9) flat in vec4 vQ;     // highlight alpha, scale (px per CSS px), glass thickness, bezel
layout(location = 10) flat in vec4 vRClip; // a clip with round corners (x0, y0, x1, y1), and its radius
layout(location = 11) flat in float vRRad;
layout(location = 12) flat in vec4 vLens; // the clip's lens: rim width, depth (0: none); uv per px (x, y)

layout(location = 0) out vec4 outColor;

// Where this pixel is drawn from, and its picture coordinates: the pixel's own, or moved by a lens (`lens`).
vec2 P;
vec2 UV;

#include "squircle.glsl"
#include "bend.glsl"

// Rounded box with a radius per corner (Apple's continuous corners, squircle.glsl); p is relative to the
// center, y down.
float sdBox(vec2 p, vec2 b, vec4 r) {
    float rad = p.x < 0.0 ? (p.y < 0.0 ? r.x : r.w) : (p.y < 0.0 ? r.y : r.z);
    return sdSquircle(p, b, rad);
}

vec4 premul(vec4 c) {
    return vec4(c.rgb * c.a, c.a);
}

// Wallpaper coordinates of screen px: "cover" fit, as the background pass.
vec2 wallUv(vec2 px) {
    vec2 uv = px / pc.screen;
    float sa = pc.screen.x / pc.screen.y;
    if (pc.bgAspect > sa) {
        float s = sa / pc.bgAspect;
        uv.x = uv.x * s + (1.0 - s) * 0.5;
    } else {
        float s = pc.bgAspect / sa;
        uv.y = uv.y * s + (1.0 - s) * 0.5;
    }
    return uv;
}

// Fill color at px (premultiplied): solid, linear / radial gradient, or picture. CSS interpolates gradients in
// premultiplied space too.
vec4 fill(vec2 px) {
    if (vP.z < 0.5) return premul(vC0);
    // Picture, cover-fitted to the box (src/ui/image.rs); vC0: color multiplier (opacity, darkening).
    // (u from 2 on: the cover's own texture, src/ui/mod.rs `picture_uv`.)
    if (vP.z > 2.5) return (UV.x >= 1.5 ? texture(uCover, UV - vec2(2.0, 0.0)) : texture(uImg, UV)) * vec4(vC0.rgb * vC0.a, vC0.a);
    float t;
    if (vP.z < 1.5) {
        vec2 d = vGrad.zw - vGrad.xy;
        t = dot(px - vGrad.xy, d) / max(dot(d, d), 1e-6);
    } else {
        t = length(px - vGrad.xy) / vGrad.z;
    }
    return mix(premul(vC0), premul(vC1), clamp(t, 0.0, 1.0));
}

// The wallpaper behind px ("cover" fit, as the background pass), blurred ~1.5 CSS px, saturate(1.6),
// brightness(1.02). Four bilinear taps half a blur radius apart.
vec3 wallFrost(vec2 px) {
    vec2 uv = wallUv(px);
    vec3 c;
    if (pc.frost > 0.0) {
        // A gaussian of sigma `frost` px from the background's mips (see-through: it is sharp). Fast: 4 taps
        // ~0.7 sigma out on the level whose texel is about sigma. Fine: a 4 x 4 grid, gaussian-weighted, on the
        // level a step finer (its boxes shorter than the kernel: no blocks).
        vec2 tex = vec2(textureSize(uWall, 0));
        float s = pc.frost * max(tex.x / pc.screen.x, tex.y / pc.screen.y); // in texels
        vec2 t = pc.frost / pc.screen;                                      // sigma in uv (screen-sized)
        if (pc.frostQ > 0.5) {
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
            float lod = max(log2(s) - 0.5, 0.0);
            vec2 o = 0.7 * t;
            c = 0.25 * (textureLod(uWall, uv + vec2(-o.x, -o.y), lod).rgb + textureLod(uWall, uv + vec2(o.x, -o.y), lod).rgb
                      + textureLod(uWall, uv + vec2(-o.x, o.y), lod).rgb + textureLod(uWall, uv + vec2(o.x, o.y), lod).rgb);
        }
    } else {
        vec2 o = vec2(0.75 * vQ.y) / pc.screen;
        c = 0.25 * (texture(uWall, uv + vec2(-o.x, -o.y)).rgb + texture(uWall, uv + vec2(o.x, -o.y)).rgb
                  + texture(uWall, uv + vec2(-o.x, o.y)).rgb + texture(uWall, uv + vec2(o.x, o.y)).rgb);
    }
    float lum = dot(c, vec3(0.2126, 0.7152, 0.0722));
    return clamp((lum + (c - lum) * 1.6) * 1.02, 0.0, 1.0);
}

vec3 frost(vec2 px) {
    // Glass over the video (fill type 7, vGrad: the video's box, corners): the video under it, blurred
    // ~3 CSS px, saturate(1.28) brightness(1.04) (the web's `.canvas-meta-glass`). It fades out toward its
    // bottom as the video is drawn (the player's last 16.2% of the screen height); below it, the darkened
    // wallpaper the page shows there (not the video's last row stretched).
    if (KIND == 2 && vP.z > 6.5) {
        float fadeLen = 0.162 * pc.screen.y;
        float a = 0.94 * (1.0 - smoothstep(vGrad.w - fadeLen, vGrad.w, px.y));
        if (a <= 0.0) {
            return wallFrost(px) * 0.5;
        }
        vec2 vs = max(vGrad.zw - vGrad.xy, vec2(1.0));
        vec2 vuv = (px - vGrad.xy) / vs;
        vec2 vo = vec2(1.5 * vQ.y) / vs;
        vec3 v = 0.25 * (texture(uVideo, vuv + vec2(-vo.x, -vo.y)).rgb + texture(uVideo, vuv + vec2(vo.x, -vo.y)).rgb
                       + texture(uVideo, vuv + vec2(-vo.x, vo.y)).rgb + texture(uVideo, vuv + vec2(vo.x, vo.y)).rgb);
        v = videoLook(v);
        float vl = dot(v, vec3(0.2126, 0.7152, 0.0722));
        vec3 vf = clamp((vl + (v - vl) * 1.28) * 1.04, 0.0, 1.0);
        return a >= 0.94 ? vf : mix(wallFrost(px) * 0.5, vf, a / 0.94);
    }
    return wallFrost(px);
}

// What the glass shows through: the frosted wallpaper (or video), or nothing where the wallpaper would be when DWM
// draws the glass under the window (frostQ 2): the window is see-through there.
vec4 backdrop(vec2 px) {
    if (pc.frostQ > 1.5 && !(KIND == 2 && vP.z > 6.5)) return vec4(0.0);
    return vec4(frost(px), 1.0);
}

float erfApprox(float x) {
    float x2 = x * x;
    float k = 0.147;
    return sign(x) * sqrt(1.0 - exp(-x2 * (1.2732395 + k * x2) / (1.0 + k * x2)));
}

vec4 over(vec4 top, vec4 bottom) {
    return top + bottom * (1.0 - top.a);
}

// Bezel height profile: 0 at the edge, 1 where the flat top starts (same as glass.frag).
float surfaceHeight(float t) {
    float s = 1.0 - t;
    return pow(1.0 - s * s * s * s, 0.25);
}

const float IOR = 3.0;
const float SPEC = 0.55;

// Liquid glass: the blurred wallpaper refracted by the bezel, the tint `tintCol` (premultiplied) over it,
// and a crisp specular rim. p: position from the center, hb: half size, sd: signed distance (< 0 inside).
vec4 glass(vec2 p, vec2 hb, float sd, vec4 tintCol) {
    float s = vQ.y;
    float rmax = max(max(vRadii.x, vRadii.y), max(vRadii.z, vRadii.w));
    float d = -sd;
    // (At most the corners' radius, or half the bezel asked for when that is more: a PC's wider rim.)
    float bezel = max(1.0, min(vQ.w, min(max(rmax, vQ.w * 0.5), min(hb.x, hb.y)) - 1.0));
    // Flat top (most of the card): no refraction, no rim, no inner shadow.
    if (d >= bezel) return over(tintCol, backdrop(P));
    float t = clamp(d / bezel, 0.0, 1.0);
    float h = surfaceHeight(t);
    float dh = (surfaceHeight(min(t + 0.001, 1.0)) - h) / 0.001;
    float slope = atan(dh * (vQ.z / bezel));
    float thetaR = asin(clamp(sin(slope) / IOR, -1.0, 1.0));
    float displacement = bendDisplacement(d, bezel, vQ.z, IOR);
    vec2 grad = normalize(vec2(sdBox(p + vec2(0.5, 0.0), hb, vRadii) - sd, sdBox(p + vec2(0.0, 0.5), hb, vRadii) - sd) + 1e-6);

    vec4 under = backdrop(P - grad * displacement);
    float innerShadow = 1.0 - smoothstep(0.0, bezel * 0.6, d);
    under.rgb *= mix(1.0, 0.7, innerShadow * 0.3);
    vec4 c = over(tintCol, under);

    vec2 lightDir = normalize(vec2(0.5, -0.7));
    float facing = abs(dot(grad, lightDir));
    float line = 1.0 - smoothstep(0.0, 1.6 * s, d);
    float glow = 1.0 - smoothstep(0.0, bezel * 0.25, d);
    float spec = clamp(SPEC * (line * mix(0.35, 1.0, facing * facing) + 0.35 * pow(facing * glow, 2.0)), 0.0, 1.0);
    c.rgb = 1.0 - (1.0 - c.rgb) * (1.0 - spec);
    return c;
}

// Signed distance to a quad (any simple polygon of 4 corners; two may coincide: a triangle).
float sdQuad(vec2 p, vec2 v0, vec2 v1, vec2 v2, vec2 v3) {
    vec2 v[4] = vec2[4](v0, v1, v2, v3);
    float d = dot(p - v[0], p - v[0]);
    float s = 1.0;
    for (int i = 0, j = 3; i < 4; j = i, i++) {
        vec2 e = v[j] - v[i];
        vec2 w = p - v[i];
        vec2 b = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-6), 0.0, 1.0);
        d = min(d, dot(b, b));
        bvec3 c = bvec3(p.y >= v[i].y, p.y < v[j].y, e.x * w.y > e.y * w.x);
        if (all(c) || all(not(c))) s = -s;
    }
    return s * sqrt(d);
}

// How much of this pixel is inside a clip box with round corners `b` (x0, y0, x1, y1), radius `r`.
float roundCov(vec4 b, float r) {
    vec2 hb = (b.zw - b.xy) * 0.5;
    return clamp(0.5 - sdBox(vPx - (b.xy + hb), hb, vec4(r)), 0.0, 1.0);
}

// A lens along the top and bottom edges of a clip box `b` (a scrolling column: what scrolls in comes and goes
// there): a glass rim `bezel` px tall. Only what lies under the rim itself is bent, squeezed toward the edge as
// through the rim of thick glass (more of it the deeper `thick` is); nothing from further in is shown there
// again. A pixel at depth d into the rim shows what is at depth bezel * f(d / bezel), f(t) = t + k t (1 - t)^2:
// 0 at the edge, 1 (and as steep as outside) where the rim ends, rising all along it (k < 3), so the picture
// stays whole. What is drawn is evaluated at that point (P, UV). (`r`, the corners, are left to the clip.)
void lens(vec4 b, float r, float bezel, float thick) {
    float top = vPx.y - b.y;
    float bottom = b.w - vPx.y;
    float d = min(top, bottom);
    if (d <= 0.0 || d >= bezel) return;
    float t = d / bezel;
    float k = clamp(thick / bezel, 0.0, 2.5);
    float u = 1.0 - t;
    float s = bezel * (t + k * t * u * u);
    // Further in: down at the top, up at the bottom.
    vec2 off = vec2(0.0, top < bottom ? s - d : d - s);
    P = vPx + off;
    UV = vUv + off * vLens.zw;
}

void shade();

void main() {
    P = vPx;
    UV = vUv;
    // (The lens of the box this is clipped by; else the one along a PC's page panel.)
    if (vRRad > 0.0 && vLens.x > 0.0) lens(vRClip, vRRad, vLens.x, vLens.y);
    else if (pc.roundR > 0.0 && pc.lens > 0.0) lens(pc.round, pc.roundR, pc.lens, pc.lens * 1.4);
    shade();
    // (Rounded clips: a PC's panels, and boxes that clip what is in them.)
    float k = 1.0;
    if (vRRad > 0.0) k = roundCov(vRClip, vRRad);
    if (pc.roundR > 0.0) k *= roundCov(pc.round, pc.roundR);
    outColor *= k;
}

void shade() {
    // Kind 4: the rare shapes, in a pipeline of their own (a branch for them in kind 0 slows every plain shape
    // on Mali by a third). Fill type 5: a vector quad (shape morphs: its corners move every frame), rounded by
    // vRadii.x. Fill type 6: the video, faded out toward its bottom between vGrad.y and vGrad.w (the web's
    // canvas mask), vC0.a opacity. Otherwise: a rounded box of the bare wallpaper (a page's backing in
    // transitions), vC0.a opacity.
    if (KIND == 4) {
        float d;
        if (vP.z > 5.5) {
            vec2 hb6 = vRect.zw * 0.5;
            d = sdBox(P - (vRect.xy + hb6), hb6, vRadii);
            float a = vC0.a * 0.94 * (1.0 - smoothstep(vGrad.y, vGrad.w, P.y));
            outColor = vec4(videoLook(texture(uVideo, UV).rgb) * a, a);
        } else if (vP.z > 4.5) {
            d = sdQuad(P, vGrad.xy, vGrad.zw, vBorder.xy, vBorder.zw) - vRadii.x;
            outColor = premul(vC0);
        } else {
            vec2 hb4 = vRect.zw * 0.5;
            d = sdBox(P - (vRect.xy + hb4), hb4, vRadii);
            outColor = (pc.frostQ > 1.5 ? vec4(0.0) : vec4(texture(uWall, wallUv(P)).rgb, 1.0)) * vC0.a;
        }
        float cov4 = clamp(0.5 - d, 0.0, 1.0);
        if (cov4 <= 0.0) discard;
        outColor *= cov4;
        return;
    }
    if (KIND == 1) {
        // A mask is its quad exactly: a quad grown for a lens (or a point a lens moved) past it would read the
        // atlas's neighbors.
        if (any(lessThan(P, vRect.xy)) || any(greaterThan(P, vRect.xy + vRect.zw))) discard;
        outColor = fill(P) * texture(uAtlas, UV).r;
        return;
    }
    vec2 hb = vRect.zw * 0.5;
    vec2 p = P - (vRect.xy + hb);
    float sd = sdBox(p, hb, vRadii);
    if (KIND == 3) {
        // Under the shape itself (shadow rect moved up by its offset) the opaque shape covers the shadow.
        if (sdBox(p + vec2(0.0, vQ.x), hb, vRadii) < -1.0) discard;
        float sigma = max(vP.w * 0.5, 0.5);
        float a = 0.5 - 0.5 * erfApprox(sd / (sigma * 1.4142136));
        outColor = premul(vC0) * a;
        return;
    }
    float cov = clamp(0.5 - sd, 0.0, 1.0);
    if (cov <= 0.0) discard;
    // Plain shapes: away from the edge, border and highlight there is only the fill.
    if (KIND == 0 && sd < -(max(vP.x, vQ.x > 0.0 ? vQ.y * 1.5 : 0.0) + 1.0)) {
        outColor = fill(P);
        return;
    }

    vec4 col = KIND == 2 && vP.z > 6.5 ? premul(vC0) : fill(P);
    if (KIND == 2) col = glass(p, hb, sd, col) * vP.w; // vP.w: the glass's opacity
    if (vQ.x > 0.0) {
        // CSS `inset 1px 1px 0 white(a)`: the part of the box not covered by itself shifted right-down.
        float sd2 = sdBox(p - vec2(vQ.y), hb, vRadii);
        float band = 1.0 - clamp(0.5 - sd2, 0.0, 1.0);
        col = over(vec4(vQ.x * band), col);
    }
    float bw = vP.x;
    if (bw > 0.0) {
        float inner = clamp(0.5 - (sd + bw), 0.0, 1.0);
        col = over(premul(vBorder) * (1.0 - inner), col);
    }
    outColor = col * cov;
}
