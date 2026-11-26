#version 450
#extension GL_GOOGLE_include_directive : require
// Live wallpaper: one particle's move this step (x0, y0, x1, y1, physical px) as a thin quad,
// `width` wide plus a pixel of edge for the coverage ramp (lw_seg.frag).
#include "livewall.glsl"
layout(location = 0) in vec4 seg;
layout(location = 0) out float vAcross;
layout(location = 1) out float vAlong;
layout(location = 2) out float vLen;
void main() {
    vec2 a = seg.xy, b = seg.zw;
    vec2 d = b - a;
    float l = length(d);
    vec2 dir = l > 1e-4 ? d / l : vec2(1.0, 0.0);
    vec2 n = vec2(-dir.y, dir.x);
    float hw = pc.width * 0.5 + 1.0;
    int i = gl_VertexIndex;
    float along = i < 2 ? -1.0 : l + 1.0;
    float across = (i & 1) == 0 ? -hw : hw;
    vec2 p = a + dir * along + n * across;
    vAcross = across;
    vAlong = along;
    vLen = l;
    gl_Position = vec4(p / pc.size * 2.0 - 1.0, 0.0, 1.0);
}
