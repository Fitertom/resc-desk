#!/usr/bin/env python3
"""Windows cursors (.cur/.ani, e.g. a copy of C:\\Windows\\Cursors) → an Xcursor theme.

    scripts/win-cursors.py SRC_DIR THEME_NAME [OUT_ROOT]

Writes OUT_ROOT/THEME_NAME/cursors/* and index.theme (OUT_ROOT: ~/.local/share/icons).
Windows draws a 32 px cursor at 100 %: that image becomes nominal size 24 (what X programs
and resc-desk ask for by default), 48 px → 36, 64 → 48, 96 → 72, 128 → 96. Inverting
pixels of 1-bit cursors (the text beam, the cross) become white with a black rim.
No dependencies beyond Python 3.
"""
import os
import struct
import sys
import zlib

# Xcursor names → the Windows file (the default scheme), best first.
MAP = {
    "aero_arrow": ["default", "left_ptr", "arrow", "top_left_arrow", "left_arrow", "context-menu", "copy", "alias",
                   "dnd-none", "dnd-move", "dnd-copy", "dnd-link", "dnd-ask", "center_ptr", "right_ptr", "draft"],
    "aero_helpsel": ["help", "question_arrow", "whats_this", "left_ptr_help", "dnd-ask"],
    "aero_working": ["progress", "left_ptr_watch", "half-busy"],
    "aero_busy": ["wait", "watch"],
    "cross_r": ["crosshair", "cross", "tcross", "cross_reverse", "diamond_cross", "cell", "plus", "color-picker"],
    "beam_r": ["text", "xterm", "ibeam", "vertical-text"],
    "aero_pen": ["pencil"],
    "aero_unavail": ["not-allowed", "no-drop", "crossed_circle", "forbidden", "circle", "X_cursor", "pirate", "dnd-no-drop"],
    "aero_ns": ["ns-resize", "n-resize", "s-resize", "size_ver", "sb_v_double_arrow", "v_double_arrow", "top_side",
                "bottom_side", "row-resize", "split_v", "sb_up_arrow", "sb_down_arrow"],
    "aero_ew": ["ew-resize", "e-resize", "w-resize", "size_hor", "sb_h_double_arrow", "h_double_arrow", "left_side",
                "right_side", "col-resize", "split_h", "sb_left_arrow", "sb_right_arrow"],
    "aero_nwse": ["nwse-resize", "nw-resize", "se-resize", "size_fdiag", "bd_double_arrow", "top_left_corner",
                  "bottom_right_corner", "ul_angle", "lr_angle"],
    "aero_nesw": ["nesw-resize", "ne-resize", "sw-resize", "size_bdiag", "fd_double_arrow", "top_right_corner",
                  "bottom_left_corner", "ur_angle", "ll_angle"],
    "aero_move": ["move", "fleur", "size_all", "all-scroll", "grabbing", "closedhand", "dnd-move"],
    "aero_link": ["pointer", "hand", "hand1", "hand2", "pointing_hand", "grab", "openhand"],
    "aero_up": ["up_arrow", "up-arrow"],
}


def png_rgba(data):
    """A PNG (8-bit RGB/RGBA, as Windows cursors hold) → (w, h, [ARGB straight])."""
    pos, chunks = 8, []
    while pos < len(data):
        n, = struct.unpack(">I", data[pos:pos + 4])
        t = data[pos + 4:pos + 8]
        chunks.append((t, data[pos + 8:pos + 8 + n]))
        pos += 12 + n
    ihdr = dict(chunks)[b"IHDR"]
    w, h, depth, ctype = struct.unpack(">IIBB", ihdr[:10])
    if depth != 8 or ctype not in (2, 6):
        raise ValueError("png kind")
    ch = 4 if ctype == 6 else 3
    raw = zlib.decompress(b"".join(d for t, d in chunks if t == b"IDAT"))
    stride = w * ch
    prev = bytearray(stride)
    out = []
    i = 0
    for _ in range(h):
        f = raw[i]
        line = bytearray(raw[i + 1:i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - ch] if x >= ch else 0
            b = prev[x]
            c = prev[x - ch] if x >= ch else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        for x in range(w):
            r, g, bb = line[x * ch], line[x * ch + 1], line[x * ch + 2]
            al = line[x * ch + 3] if ch == 4 else 255
            out.append((al << 24) | (r << 16) | (g << 8) | bb)
        prev = line
    return w, h, out


def dib_rgba(d, w, h):
    """A cursor's DIB (XOR image then AND mask, bottom-up) → straight ARGB."""
    hs, = struct.unpack("<I", d[:4])
    bw, bh2, _, bpp = struct.unpack("<iiHH", d[4:16])
    ncol, = struct.unpack("<I", d[32:36])
    if bpp <= 8:
        ncol = ncol or (1 << bpp)
    else:
        ncol = 0
    pal = [struct.unpack("<BBBx", d[hs + 4 * i:hs + 4 * i + 4]) for i in range(ncol)]
    xo = hs + 4 * ncol
    xstride = ((w * bpp + 31) // 32) * 4
    ao = xo + xstride * h
    astride = ((w + 31) // 32) * 4
    px = [0] * (w * h)
    inv = [False] * (w * h)
    any_alpha = bpp == 32 and any(d[xo + i * 4 + 3] for i in range(w * h) if xo + i * 4 + 3 < len(d))
    for y in range(h):
        sy = h - 1 - y
        for x in range(w):
            m = (d[ao + sy * astride + x // 8] >> (7 - x % 8)) & 1 if ao + sy * astride + x // 8 < len(d) else 0
            if bpp == 32:
                b, g, r, a = d[xo + sy * xstride + x * 4: xo + sy * xstride + x * 4 + 4]
                if not any_alpha:
                    a = 0 if m else 255
                px[y * w + x] = (a << 24) | (r << 16) | (g << 8) | b
                continue
            if bpp == 24:
                b, g, r = d[xo + sy * xstride + x * 3: xo + sy * xstride + x * 3 + 3]
            else:
                bit = x * bpp
                v = (d[xo + sy * xstride + bit // 8] >> (8 - bpp - bit % 8)) & ((1 << bpp) - 1)
                b, g, r = pal[v] if v < len(pal) else (0, 0, 0)
            if m:
                # AND 1: transparent, unless the XOR colour inverts what is under it.
                if (r, g, b) != (0, 0, 0):
                    inv[y * w + x] = True
                continue
            px[y * w + x] = (255 << 24) | (r << 16) | (g << 8) | b
    if any(inv):
        # Inverting pixels: white, a black rim around them where nothing else is.
        for i, v in enumerate(inv):
            if v:
                px[i] = 0xFFFFFFFF
        for y in range(h):
            for x in range(w):
                if px[y * w + x] >> 24:
                    continue
                near = any(0 <= x + dx < w and 0 <= y + dy < h and inv[(y + dy) * w + x + dx]
                           for dx in (-1, 0, 1) for dy in (-1, 0, 1))
                if near:
                    px[y * w + x] = 0xFF000000
    return px


def cur_images(b):
    """A .cur/.ico → [(w, h, hx, hy, straight ARGB)]."""
    _, typ, n = struct.unpack("<HHH", b[:6])
    out = []
    for i in range(n):
        w, h, _, _, hx, hy, size, off = struct.unpack("<BBBBHHII", b[6 + 16 * i:22 + 16 * i])
        w, h = w or 256, h or 256
        d = b[off:off + size]
        try:
            if d[:4] == b"\x89PNG":
                pw, ph, px = png_rgba(d)
                w, h = pw, ph
            else:
                px = dib_rgba(d, w, h)
        except Exception as e:  # noqa: BLE001 — skip an image we cannot read
            print(f"  skipped a {w}x{h} image: {e}", file=sys.stderr)
            continue
        if typ != 2:
            hx = hy = 0
        out.append((w, h, hx, hy, px))
    # One image per size: the deepest (32-bit before 8-bit) comes first in these files.
    seen, uniq = set(), []
    for im in out:
        if im[0] not in seen:
            seen.add(im[0])
            uniq.append(im)
    return uniq


def ani_frames(b):
    """A .ani → ([frames: image lists], [delay ms per step], [frame index per step])."""
    frames, rate, seq, jif = [], None, None, 6
    pos = 12
    def walk(p, end):
        nonlocal rate, seq, jif
        while p + 8 <= end:
            cid = b[p:p + 4]
            n, = struct.unpack("<I", b[p + 4:p + 8])
            body = b[p + 8:p + 8 + n]
            if cid == b"anih":
                jif = struct.unpack("<9I", body[:36])[7] or 6
            elif cid == b"rate":
                rate = list(struct.unpack(f"<{n // 4}I", body))
            elif cid == b"seq ":
                seq = list(struct.unpack(f"<{n // 4}I", body))
            elif cid == b"LIST":
                walk(p + 12, p + 8 + n)
            elif cid == b"icon":
                frames.append(cur_images(body))
            p += 8 + n + (n & 1)
    walk(pos, len(b))
    steps = seq or list(range(len(frames)))
    rates = rate or [jif] * len(steps)
    return frames, [max(1, round(r * 1000 / 60)) for r in rates], steps


def premul(c):
    a = c >> 24
    if a == 255:
        return c
    r, g, b = (c >> 16) & 255, (c >> 8) & 255, c & 255
    return (a << 24) | ((r * a + 127) // 255 << 16) | ((g * a + 127) // 255 << 8) | (b * a + 127) // 255


def xcursor(images):
    """[(nominal, w, h, hx, hy, delay, straight ARGB)] → an Xcursor file."""
    hdr = struct.pack("<4sIII", b"Xcur", 16, 0x10000, len(images))
    toc, chunks = b"", b""
    pos = 16 + 12 * len(images)
    for nom, w, h, hx, hy, delay, px in images:
        c = struct.pack("<IIIIIIIII", 36, 0xFFFD0002, nom, 1, w, h, hx, hy, delay)
        c += struct.pack(f"<{w * h}I", *[premul(p) for p in px])
        toc += struct.pack("<III", 0xFFFD0002, nom, pos)
        chunks += c
        pos += len(c)
    return hdr + toc + chunks


def nominal(px):
    return px * 3 // 4


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    src, name = sys.argv[1], sys.argv[2]
    root = sys.argv[3] if len(sys.argv) > 3 else os.path.expanduser("~/.local/share/icons")
    out = os.path.join(root, name, "cursors")
    os.makedirs(out, exist_ok=True)
    for base, names in MAP.items():
        path = next((os.path.join(src, base + e) for e in (".ani", ".cur") if os.path.exists(os.path.join(src, base + e))), None)
        if not path:
            print(f"{base}: not found, skipped", file=sys.stderr)
            continue
        b = open(path, "rb").read()
        imgs = []
        if b[:4] == b"RIFF":
            frames, delays, steps = ani_frames(b)
            sizes = sorted({im[0] for f in frames for im in f})
            for s in sizes:
                for k, fi in enumerate(steps):
                    im = next((i for i in frames[fi] if i[0] == s), None) if fi < len(frames) else None
                    if im:
                        imgs.append((nominal(s), im[0], im[1], im[2], im[3], delays[k % len(delays)], im[4]))
        else:
            for w, h, hx, hy, px in cur_images(b):
                imgs.append((nominal(w), w, h, hx, hy, 0, px))
        data = xcursor(imgs)
        first = os.path.join(out, names[0])
        open(first, "wb").write(data)
        for n in names[1:]:
            link = os.path.join(out, n)
            if os.path.lexists(link):
                os.remove(link)
            os.symlink(names[0], link)
        print(f"{base} → {names[0]} (+{len(names) - 1} names), {len(imgs)} images")
    with open(os.path.join(root, name, "index.theme"), "w") as f:
        f.write(f"[Icon Theme]\nName={name}\nComment=Windows cursors\nInherits=Adwaita\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
