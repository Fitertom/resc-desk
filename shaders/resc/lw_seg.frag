#version 450
#extension GL_GOOGLE_include_directive : require
// Live wallpaper: a segment's coverage (1 px wide line, its ends rounded off by a pixel).
#include "livewall.glsl"
layout(location = 0) in float vAcross;
layout(location = 1) in float vAlong;
layout(location = 2) in float vLen;
layout(location = 0) out vec4 outColor;
void main() {
    float c = clamp(pc.width * 0.5 + 0.5 - abs(vAcross), 0.0, 1.0);
    float e = clamp(0.5 + min(vAlong, vLen - vAlong) + 0.5, 0.0, 1.0);
    outColor = pc.color * c * e;
}
