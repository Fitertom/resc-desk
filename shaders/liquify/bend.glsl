// The bezel's bending without a fold (resc-desk). The physical lens of glass.frag / ui.frag
// shows, at depth d into the bezel, what lies at d + displacement(d); right at the edge that
// is far inside, and it comes nearer going in, down to the nearest at some depth, then goes
// back out: past that turn the picture shows twice, mirrored, with a crease along the seam
// (plain on clear glass over a sharp picture). Here it turns smoothly instead: from the turn
// to the edge the depth shown keeps getting smaller (the picture squeezed toward the edge),
// meeting the lens at the turn with the same slope. Same lens past the turn.

float bendHeight(float t) {
    float s = 1.0 - t;
    return pow(1.0 - s * s * s * s, 0.25);
}

// What the lens shows at depth d: the depth it is from (d + its displacement).
float bendLensDepth(float d, float bezel, float thick, float ior) {
    float t = clamp(d / bezel, 0.0, 1.0);
    float h = bendHeight(t);
    float dh = (bendHeight(min(t + 0.001, 1.0)) - h) / 0.001;
    float slope = atan(dh * (thick / bezel));
    float thetaR = asin(clamp(sin(slope) / ior, -1.0, 1.0));
    return d + h * thick * (tan(slope) - tan(thetaR));
}

// The displacement at depth d (toward the inside), folding nowhere.
float bendDisplacement(float d, float bezel, float thick, float ior) {
    // The turn: the nearest depth any point of the bezel shows (24 steps, then 8 finer).
    float dt = bezel, Dt = 1e9;
    for (int i = 1; i <= 24; i++) {
        float x = bezel * float(i) / 24.0;
        float D = bendLensDepth(x, bezel, thick, ior);
        if (D < Dt) { Dt = D; dt = x; }
    }
    float step = bezel / 24.0;
    float lo = max(dt - step, 0.01);
    for (int i = 0; i <= 8; i++) {
        float x = lo + 2.0 * step * float(i) / 8.0;
        float D = bendLensDepth(x, bezel, thick, ior);
        if (D < Dt) { Dt = D; dt = x; }
    }
    if (d >= dt) return bendLensDepth(d, bezel, thick, ior) - d;
    // Edge side of the turn: Dt at the turn (flat there, as the lens), down to Dt - dt at the
    // edge (never outside the shape).
    float D0 = max(Dt - dt, 0.0);
    float u = 1.0 - d / max(dt, 1e-3);
    return Dt - (Dt - D0) * u * u - d;
}
