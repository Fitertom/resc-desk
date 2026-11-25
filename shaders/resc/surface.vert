#version 450
// A textured rect (a client surface, a window's frame and shadow) or a solid one
// (single-pixel buffers), physical pixels. Triangle strip, 4 vertices.

layout(push_constant) uniform PC {
    vec2 screen;
    vec4 dst;   // x, y, w, h on the output
    vec4 uv;    // u0, v0, u1, v1 in the texture
    vec4 color; // solid fill, premultiplied (mode 1)
    float mode; // 0: texture, 1: color
    float alpha;
    vec4 clip;    // rounded clip on the output (x, y, w, h): a window's corners
    float radius;  // its top corners' radius; 0 (and radiusB 0): no clip
    float radiusB; // its bottom corners' radius
} pc;

layout(location = 0) out vec2 vUv;

void main() {
    vec2 c = vec2(float(gl_VertexIndex & 1), float((gl_VertexIndex >> 1) & 1));
    vec2 px = pc.dst.xy + c * pc.dst.zw;
    vUv = mix(pc.uv.xy, pc.uv.zw, c);
    gl_Position = vec4(px / pc.screen * 2.0 - 1.0, 0.0, 1.0);
}
