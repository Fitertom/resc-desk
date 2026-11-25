// Rounded corners the way Apple draws them ("continuous" corners): the curve starts earlier than a circular
// arc of the same radius would and its curvature grows from zero, so there is no visible kink where a
// straight edge meets the corner. A superellipse quarter |x/Rx|^n + |y/Ry|^n = 1 per corner (zero
// curvature where it meets the edges for n > 2). Capsules (radius = half the short side) get an elliptic
// cap a little longer than a half circle instead: n = 4-ish there would flatten the tip into a square end.
// Distance: exact along straight edges, first-order (value / gradient) in the corners.

// Signed distance to a box of half size `b` with corner radius `r` (< 0 inside), p from the center.
float sdSquircle(vec2 p, vec2 b, float r) {
    vec2 a = abs(p);
    r = min(r, min(b.x, b.y));
    if (r <= 0.5) {
        vec2 d = a - b;
        return min(max(d.x, d.y), 0.0) + length(max(d, 0.0));
    }
    float cap = smoothstep(0.8, 1.0, r / min(b.x, b.y));
    // (A circle stays a circle: a square box rounded all the way.)
    float round = cap * smoothstep(0.9, 1.0, min(b.x, b.y) / max(b.x, b.y));
    float n = mix(mix(2.8, 2.25, cap), 2.0, round);
    vec2 R = min(vec2(r * mix(1.6, 1.12, cap)), b);
    vec2 q = a - (b - R); // from where the corner's curve starts
    if (q.x <= 0.0 || q.y <= 0.0) return max(a.x - b.x, a.y - b.y);
    vec2 u = q / R;
    float s = pow(pow(u.x, n) + pow(u.y, n), 1.0 / n);
    vec2 g = pow(u / s, vec2(n - 1.0)) / R;
    return (s - 1.0) / max(length(g), 1e-6);
}
