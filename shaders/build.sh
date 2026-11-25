#!/bin/sh
# Compiles the shaders to SPIR-V (shaders/spv/*.spv, committed: building resc-desk needs no
# shader compiler). Needs glslangValidator (Debian/Ubuntu: glslang-tools).
# Run after changing anything in shaders/.
set -e
cd "$(dirname "$0")"
mkdir -p spv
L=liquify
c() { glslangValidator -V --target-env vulkan1.3 -I"$L" -o "spv/$2.spv" "$1" > /dev/null; echo "spv/$2.spv"; }
c $L/fullscreen.vert fullscreen.vert
c $L/glass.vert glass.vert
c $L/glass.frag glass.frag
c $L/ui.vert ui.vert
c $L/ui.frag ui.frag
c resc/layer.frag layer.frag
c resc/blur.frag blur.frag
c resc/shadow.frag shadow.frag
c resc/surface.vert surface.vert
c resc/surface.frag surface.frag
c resc/drop.vert drop.vert
c resc/drop.frag drop.frag
if command -v spirv-val > /dev/null; then
    for f in spv/*.spv; do spirv-val --target-env vulkan1.3 "$f"; done
fi
