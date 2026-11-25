// Shared by every shader: push constants, background sampling, rounded-rect SDF.
// The push-constant block must match `PushConsts` in src/gpu.rs (std430, 104 bytes).

layout(push_constant) uniform PC {
    vec2 screen;
    vec2 center;
    vec2 halfSize;
    float radius;
    float thickness;
    float bezel;
    float ior;
    float blur;
    float spec;
    float tint;
    float shadow;
    float disp;
    float sat;
    float bgAspect;
    float scale;
    float bgMode;   // 0 wallpaper, 1 black, 2 white, 3 checker, 4 backdrop (copy of the screen strip)
    float lod;      // backdrop: mip level to sample (blur)
    vec4 region;    // backdrop: screen rect it was copied from (x, y, w, h), physical px
    float bgFade;   // a crossfade from the previous background: how much of it is still shown (0: none)
    float prevAspect;
} pc;

// Glass quad covers the shape plus this margin (physical px = MARGIN * scale). Beyond it the shadow is
// below 1/100 of full intensity (invisible), and the quad stays small: fragments are the cost on phones.
const float GLASS_MARGIN = 56.0;
const float SHADOW_DROP = 6.0;

#include "squircle.glsl"

float sdRoundedRect(vec2 p, vec2 b, float r) {
    return sdSquircle(p, b, r);
}

#ifdef NEEDS_BACKGROUND
layout(set = 0, binding = 0) uniform sampler2D uBg;
layout(set = 0, binding = 1) uniform sampler2D uBgPrev; // the background being faded out

// "cover" fit of a picture of aspect `aspect` (w / h) over the screen.
vec2 coverUv(vec2 uv, float aspect) {
    float sa = pc.screen.x / pc.screen.y;
    if (aspect > sa) {
        float s = sa / aspect;
        uv.x = uv.x * s + (1.0 - s) * 0.5;
    } else {
        float s = aspect / sa;
        uv.y = uv.y * s + (1.0 - s) * 0.5;
    }
    return uv;
}

// Background with "cover" fit, like CSS background-size: cover. Backdrop: the screen strip under the glass.
vec3 sampleBg(vec2 uv) {
    if (pc.bgMode > 3.5) return textureLod(uBg, (uv * pc.screen - pc.region.xy) / pc.region.zw, pc.lod).rgb;
    if (pc.bgMode > 0.5 && pc.bgMode < 1.5) return vec3(0.0);
    if (pc.bgMode > 1.5 && pc.bgMode < 2.5) return vec3(1.0);
    if (pc.bgMode > 2.5) {
        vec2 q = uv * pc.screen / (40.0 * pc.scale);
        return mix(vec3(0.08), vec3(0.85), mod(floor(q.x) + floor(q.y), 2.0));
    }
    vec3 c = textureLod(uBg, coverUv(uv, pc.bgAspect), 0.0).rgb;
    if (pc.bgFade > 0.001) c = mix(c, textureLod(uBgPrev, coverUv(uv, pc.prevAspect), 0.0).rgb, pc.bgFade);
    return c;
}
#endif
