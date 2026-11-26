// Live wallpaper (livewall.rs): one push-constant block for its passes.
layout(push_constant) uniform PC {
    vec2 size;   // the trail image (physical px)
    float width; // a segment's width (px)
    float alpha; // tint.frag: how much of the picture
    vec4 color;  // premultiplied
} pc;
