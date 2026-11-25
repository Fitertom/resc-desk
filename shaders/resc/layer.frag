#version 450
// A layer the CPU drew (window decorations, the shell's text and icons): premultiplied BGRA,
// one texel per screen pixel, over the frame (blend ONE, ONE_MINUS_SRC_ALPHA), or copied as is
// for the bottom layer. Drawn with fullscreen.vert, cut to the damage by the scissor.

layout(set = 0, binding = 0) uniform sampler2D uLayer;

layout(location = 0) out vec4 outColor;

void main() {
    outColor = texelFetch(uLayer, ivec2(gl_FragCoord.xy), 0);
}
