#version 450
#extension GL_GOOGLE_include_directive : require
// Live wallpaper: a flat colour over the trails (their fade toward the background), or a
// tiny one taken off them (blend reverse-subtract: 8-bit trails never stick at a grey).
#include "livewall.glsl"
layout(location = 0) out vec4 outColor;
void main() {
    outColor = pc.color;
}
