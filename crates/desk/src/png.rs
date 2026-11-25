//! PNG writer: RGB8, Sub filter, own deflate (LZ77 + fixed Huffman), zlib, CRC.

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// Huffman codes go MSB-first.
    fn code(&mut self, code: u32, len: u32) {
        let mut r = 0;
        for i in 0..len {
            r |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.put(r, len);
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn lit(b: &mut Bits, v: u32) {
    match v {
        0..=143 => b.code(0x30 + v, 8),
        144..=255 => b.code(0x190 + v - 144, 9),
        256..=279 => b.code(v - 256, 7),
        _ => b.code(0xc0 + v - 280, 8),
    }
}

const LBASE: [u32; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXTRA: [u32; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DEXTRA: [u32; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut b = Bits { out: Vec::with_capacity(data.len() / 4), acc: 0, n: 0 };
    b.put(1, 1); // BFINAL
    b.put(1, 2); // fixed Huffman
    const HBITS: usize = 15;
    let mut head = vec![usize::MAX; 1 << HBITS];
    let hash = |i: usize| -> usize {
        let v = (data[i] as u32) | (data[i + 1] as u32) << 8 | (data[i + 2] as u32) << 16;
        (v.wrapping_mul(2654435761) >> (32 - HBITS)) as usize
    };
    let mut i = 0;
    while i < data.len() {
        let mut best = (0usize, 0usize);
        if i + 3 <= data.len() {
            let h = hash(i);
            let cand = head[h];
            head[h] = i;
            if cand != usize::MAX && i - cand <= 32768 {
                let max = (data.len() - i).min(258);
                let mut l = 0;
                while l < max && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l >= 3 {
                    best = (l, i - cand);
                }
            }
        }
        if best.0 >= 3 {
            let (len, dist) = best;
            let li = LBASE.iter().rposition(|&x| x <= len as u32).unwrap();
            lit(&mut b, 257 + li as u32);
            b.put(len as u32 - LBASE[li], LEXTRA[li]);
            let di = DBASE.iter().rposition(|&x| x <= dist as u32).unwrap();
            b.code(di as u32, 5);
            b.put(dist as u32 - DBASE[di], DEXTRA[di]);
            // Index a few positions inside the match (cheap).
            for k in 1..len.min(4) {
                if i + k + 3 <= data.len() {
                    let h = hash(i + k);
                    head[h] = i + k;
                }
            }
            i += len;
        } else {
            lit(&mut b, data[i] as u32);
            i += 1;
        }
    }
    lit(&mut b, 256);
    b.finish()
}

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, e) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb88320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    t
}

fn chunk(out: &mut Vec<u8>, t: &[u32; 256], kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut c = 0xffff_ffffu32;
    for &x in &out[start..] {
        c = t[((c ^ x as u32) & 0xff) as usize] ^ (c >> 8);
    }
    out.extend_from_slice(&(!c).to_be_bytes());
}

/// Encodes XRGB pixels (`stride` in pixels) as PNG. `step` > 1 downsamples.
pub fn encode(px: &[u32], w: i32, h: i32, stride: usize, step: usize) -> Vec<u8> {
    let step = step.max(1);
    let (ow, oh) = ((w as usize + step - 1) / step, (h as usize + step - 1) / step);
    let mut raw = Vec::with_capacity((ow * 3 + 1) * oh);
    for y in 0..oh {
        raw.push(1); // Sub filter
        let row = &px[y * step * stride..];
        let mut prev = [0u8; 3];
        for x in 0..ow {
            let p = row[x * step];
            let c = [(p >> 16) as u8, (p >> 8) as u8, p as u8];
            for k in 0..3 {
                raw.push(c[k].wrapping_sub(prev[k]));
            }
            prev = c;
        }
    }
    let mut z = vec![0x78, 0x01];
    z.extend(deflate(&raw));
    let (mut a, mut bb) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        bb = (bb + a) % 65521;
    }
    z.extend_from_slice(&((bb << 16) | a).to_be_bytes());
    let t = crc_table();
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(ow as u32).to_be_bytes());
    ihdr.extend_from_slice(&(oh as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, &t, b"IHDR", &ihdr);
    chunk(&mut out, &t, b"IDAT", &z);
    chunk(&mut out, &t, b"IEND", &[]);
    out
}
