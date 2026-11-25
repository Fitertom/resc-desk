#version 450
// One direction of a separable gaussian over a downscaled copy of what lies under the glass
// (a mip level of the backdrop, then the horizontal result). Same size in and out: one texel
// per fragment. Drawn with fullscreen.vert, cut to the glass's zone by the scissor.
// Pairs of taps share one bilinear fetch (the weighted point between them).

layout(set = 0, binding = 0) uniform sampler2D uSrc;

layout(push_constant) uniform PC {
    vec2 dir;     // (1, 0) or (0, 1)
    float sigma;  // texels
    float radius; // texels, about 3 sigma
} pc;

layout(location = 0) out vec4 outColor;

void main() {
    vec2 size = vec2(textureSize(uSrc, 0));
    vec2 uv = gl_FragCoord.xy / size;
    vec2 step = pc.dir / size;
    float k = -0.5 / (pc.sigma * pc.sigma);
    vec4 acc = textureLod(uSrc, uv, 0.0);
    float wsum = 1.0;
    int r = int(pc.radius);
    for (int i = 1; i <= r; i += 2) {
        float a = float(i);
        float b = a + 1.0;
        float wa = exp(k * a * a);
        float wb = b <= pc.radius ? exp(k * b * b) : 0.0;
        float w = wa + wb;
        float o = (a * wa + b * wb) / w;
        acc += w * (textureLod(uSrc, uv + step * o, 0.0) + textureLod(uSrc, uv - step * o, 0.0));
        wsum += 2.0 * w;
    }
    outColor = acc / wsum;
}
