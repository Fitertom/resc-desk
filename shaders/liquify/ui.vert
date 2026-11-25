#version 450
// UI instance quad (src/ui/mod.rs `Inst`), physical pixels. Grown by the anti-aliasing / shadow margin,
// then cut to the clip rect, so clipping costs nothing in the fragment shader.

layout(location = 0) in vec4 iRect;
layout(location = 1) in vec4 iClip;
layout(location = 2) in vec4 iC0;
layout(location = 3) in vec4 iC1;
layout(location = 4) in vec4 iGrad;
layout(location = 5) in vec4 iBorder;
layout(location = 6) in vec4 iUv;
layout(location = 7) in vec4 iRadii;
layout(location = 8) in vec4 iP;
layout(location = 9) in vec4 iQ;
layout(location = 10) in vec4 iRClip;
layout(location = 11) in vec4 iRRad;

// One pipeline per kind (src/ui/mod.rs KIND_*): each variant keeps only its own work.
layout(constant_id = 0) const int KIND = 0;

layout(push_constant) uniform PC {
    vec2 screen;
    float bgAspect;
    float scrollY; // content offset (scrolling), physical px
    vec4 round;    // a clip with round corners over everything (x0, y0, x1, y1), screen px
    float roundR;  // its radius (0: none)
    float lens;    // its lens: rim width, px (0: none)
} pc;

layout(location = 0) out vec2 vPx;
layout(location = 1) out vec2 vUv;
layout(location = 2) flat out vec4 vRect;
layout(location = 3) flat out vec4 vC0;
layout(location = 4) flat out vec4 vC1;
layout(location = 5) flat out vec4 vGrad;
layout(location = 6) flat out vec4 vBorder;
layout(location = 7) flat out vec4 vRadii;
layout(location = 8) flat out vec4 vP;
layout(location = 9) flat out vec4 vQ;
layout(location = 10) flat out vec4 vRClip;
layout(location = 11) flat out float vRRad;
layout(location = 12) flat out vec4 vLens;

void main() {
    vec2 corner = vec2(float(gl_VertexIndex & 1), float((gl_VertexIndex >> 1) & 1));
    // Masks are pixel-exact; shapes need 1 px for their soft edge; shadows reach 2 sigma (blur = 2 sigma).
    float grow = KIND == 3 ? iP.w : (KIND == 1 ? 0.0 : 1.0);
    // Under a lens the rim shows what lies a little further in (at most ~0.15 x its width x its depth / width,
    // ui.frag `lens`): the quad reaches out that far.
    float lensW = 0.4 * (iRRad.x > 0.0 && iRRad.y > 0.0 ? iRRad.y : (pc.roundR > 0.0 ? pc.lens : 0.0));
    if (KIND != 3) grow = max(grow, lensW);
    vec2 off = vec2(0.0, pc.scrollY);
    vec4 rect = vec4(iRect.xy + off, iRect.zw);
    vec4 clip = iClip + off.xyxy;
    vec2 lo = max(rect.xy - grow, clip.xy);
    vec2 hi = max(min(rect.xy + rect.zw + grow, clip.zw), lo);
    vec2 px = mix(lo, hi, corner);
    if (KIND == 3) {
        // Shadows are a ring (10-vertex strip, outer / inner corners): the middle lies under the opaque shape.
        int c = (gl_VertexIndex >> 1) & 3; // corners clockwise from top-left
        vec2 sel = vec2(c == 1 || c == 2 ? 1.0 : 0.0, c >= 2 ? 1.0 : 0.0);
        float inset = max(max(max(iRadii.x, iRadii.y), max(iRadii.z, iRadii.w)), 1.0) + 1.0;
        vec2 ilo = rect.xy + min(vec2(inset), rect.zw * 0.5);
        // Inner hole: under the shape only (the shadow rect is the shape moved down by iQ.x).
        vec2 ihi = max(rect.xy + rect.zw - vec2(0.0, iQ.x) - min(vec2(inset), rect.zw * 0.5), ilo);
        px = (gl_VertexIndex & 1) == 0 ? mix(lo, hi, sel) : clamp(mix(ilo, ihi, sel), lo, hi);
    }

    vPx = px;
    vUv = mix(iUv.xy, iUv.zw, (px - rect.xy) / max(rect.zw, vec2(1e-3)));
    vRect = rect;
    vC0 = iC0;
    vC1 = iC1;
    vGrad = iP.z > 1.5 && iP.z < 4.5 ? vec4(iGrad.xy + off, iGrad.zw) : iGrad + off.xyxy;
    // Quads (fill type 5) keep their 4 corners in grad and border: both move with the content.
    vBorder = iP.z > 4.5 && iP.z < 5.5 ? iBorder + off.xyxy : iBorder;
    vRadii = iRadii;
    vP = iP;
    vQ = iQ;
    vRClip = iRClip + off.xyxy;
    vRRad = iRRad.x;
    vLens = vec4(iRRad.y, iRRad.z, (iUv.zw - iUv.xy) / max(iRect.zw, vec2(1e-3)));
    gl_Position = vec4(px / pc.screen * 2.0 - 1.0, 0.0, 1.0);
}
