#version 450
#extension GL_GOOGLE_include_directive : require
// Live wallpaper: the wallpaper's picture over the trails at `alpha` (their fade toward it).
#include "livewall.glsl"
layout(set = 0, binding = 0) uniform sampler2D uPic;
layout(location = 0) out vec4 outColor;
void main() {
    outColor = texelFetch(uPic, ivec2(gl_FragCoord.xy), 0) * pc.alpha;
}
