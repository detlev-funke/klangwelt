//! FLAC encoder (lossless, own code): blocks of 4096 frames, fixed
//! predictors of order 0–4 with Rice coding in partitions, stereo as
//! left/right, left/side, right/side or mid/side as desired – the shortest
//! per block. Plus STREAMINFO with MD5 and comments (title).

const BLOCK: usize = 4096;
const MAX_PARTITION_ORDER: u32 = 8;

// ---------------------------------------------------------------- Bits

struct Bits {
    buf: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Bits { buf: Vec::new(), acc: 0, n: 0 }
    }

    /// The lower `bits` bits of `v` (at most 32).
    fn put(&mut self, v: u64, bits: u32) {
        if bits == 0 {
            return;
        }
        debug_assert!(bits <= 32);
        self.acc = (self.acc << bits) | (v & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.buf.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    fn unary(&mut self, mut q: u64) {
        while q >= 32 {
            self.put(0, 32);
            q -= 32;
        }
        self.put(1, q as u32 + 1);
    }

    fn align(&mut self) {
        if self.n > 0 {
            self.put(0, 8 - self.n);
        }
    }
}

fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut c, &b| {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
        c
    })
}

fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |mut c, &b| {
        c ^= (b as u16) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
        c
    })
}

// ---------------------------------------------------------------- MD5

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1,
    0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453,
    0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942,
    0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
    0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d,
    0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];
const MD5_S: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];

/// MD5 (RFC 1321) – FLAC stores a checksum of the samples with it.
pub struct Md5 {
    s: [u32; 4],
    pending: Vec<u8>,
    len: u64,
}

impl Md5 {
    pub fn new() -> Self {
        Md5 { s: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], pending: Vec::with_capacity(64), len: 0 }
    }

    fn block(&mut self, chunk: &[u8]) {
        let m: Vec<u32> = chunk.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let [mut a, mut b, mut c, mut d] = self.s;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(MD5_K[i]).wrapping_add(m[g]).rotate_left(MD5_S[(i / 16) * 4 + i % 4]));
            a = t;
        }
        for (s, v) in self.s.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.pending.is_empty() {
            let take = (64 - self.pending.len()).min(data.len());
            self.pending.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.pending.len() == 64 {
                let p = std::mem::take(&mut self.pending);
                self.block(&p);
            }
        }
        let mut chunks = data.chunks_exact(64);
        for c in &mut chunks {
            self.block(c);
        }
        self.pending.extend_from_slice(chunks.remainder());
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bits = self.len.wrapping_mul(8);
        let mut tail = vec![0x80u8];
        while (self.len as usize + tail.len()) % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_le_bytes());
        self.update(&tail);
        let mut out = [0u8; 16];
        for (i, s) in self.s.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out
    }
}

// ---------------------------------------------------------------- Subframes

enum Sub {
    Constant,
    Verbatim,
    /// Order, partition order, Rice parameter per partition (15 = raw)
    Fixed { order: usize, part_order: u32, params: Vec<u32> },
}

fn residual(x: &[i64], order: usize) -> Vec<i64> {
    let n = x.len();
    let mut r = Vec::with_capacity(n.saturating_sub(order));
    for i in order..n {
        r.push(match order {
            0 => x[i],
            1 => x[i] - x[i - 1],
            2 => x[i] - 2 * x[i - 1] + x[i - 2],
            3 => x[i] - 3 * x[i - 1] + 3 * x[i - 2] - x[i - 3],
            _ => x[i] - 4 * x[i - 1] + 6 * x[i - 2] - 4 * x[i - 3] + x[i - 4],
        });
    }
    r
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// Partition `j` of `parts` in the residual (the first is shorter by the order).
fn partition(res: &[i64], n: usize, order: usize, part_order: u32, j: usize) -> &[i64] {
    let size = n >> part_order;
    let (a, b) = if j == 0 { (0, size - order) } else { (j * size - order, (j + 1) * size - order) };
    &res[a..b]
}

/// Rice parameter for a partition with sum `sum` over `count` values, estimated bits.
fn rice_estimate(sum: u64, count: u64) -> (u32, u64) {
    (0..=14u32).map(|k| (k, count * (k as u64 + 1) + (sum >> k))).min_by_key(|&(_, bits)| bits).unwrap_or((0, 0))
}

/// Best partition order and parameters (estimated) for a residual.
fn rice_plan(res: &[i64], n: usize, order: usize) -> (u64, u32, Vec<u32>) {
    // sums at the finest allowed division, then combine pairwise
    let mut top = 0;
    while top < MAX_PARTITION_ORDER && n % (1 << (top + 1)) == 0 && (n >> (top + 1)) > order {
        top += 1;
    }
    let mut sums: Vec<(u64, u64)> = (0..1usize << top)
        .map(|j| {
            let p = partition(res, n, order, top, j);
            (p.iter().map(|&v| zigzag(v)).sum(), p.len() as u64)
        })
        .collect();
    let mut best: Option<(u64, u32, Vec<u32>)> = None;
    let mut po = top;
    loop {
        let mut bits = 0u64;
        let mut params = Vec::with_capacity(sums.len());
        for &(s, c) in &sums {
            let (k, b) = rice_estimate(s, c);
            bits += 4 + b;
            params.push(k);
        }
        if best.as_ref().is_none_or(|b| bits < b.0) {
            best = Some((bits, po, params));
        }
        if po == 0 {
            break;
        }
        sums = sums.chunks(2).map(|p| (p[0].0 + p[1].0, p[0].1 + p[1].1)).collect();
        po -= 1;
    }
    best.unwrap_or((u64::MAX, 0, vec![0]))
}

/// Shortest representation of a channel: (kind, estimated bits).
fn analyze(x: &[i64], bps: u32) -> (Sub, u64) {
    let n = x.len();
    if x.iter().all(|&v| v == x[0]) {
        return (Sub::Constant, 8 + bps as u64);
    }
    let mut best = (Sub::Verbatim, 8 + n as u64 * bps as u64);
    for order in 0..=4usize.min(n.saturating_sub(1)) {
        let res = residual(x, order);
        let (bits, part_order, params) = rice_plan(&res, n, order);
        let total = 8 + order as u64 * bps as u64 + 6 + bits;
        if total < best.1 {
            best = (Sub::Fixed { order, part_order, params }, total);
        }
    }
    best
}

fn write_sub(b: &mut Bits, x: &[i64], bps: u32, sub: &Sub) {
    match sub {
        Sub::Constant => {
            b.put(0, 8);
            b.put(x[0] as u64, bps);
        }
        Sub::Verbatim => {
            b.put(0b0000_0010, 8);
            for &v in x {
                b.put(v as u64, bps);
            }
        }
        Sub::Fixed { order, part_order, params } => {
            b.put((0b0001_0000 | (*order as u64) << 1) as u64, 8);
            for &v in &x[..*order] {
                b.put(v as u64, bps);
            }
            let res = residual(x, *order);
            b.put(0, 2); // Rice with 4-bit parameters
            b.put(*part_order as u64, 4);
            for (j, &k) in params.iter().enumerate() {
                let p = partition(&res, x.len(), *order, *part_order, j);
                let rice: u64 = p.iter().map(|&v| (zigzag(v) >> k) + 1 + k as u64).sum();
                let max = p.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
                let raw_bits = if max == 0 { 0 } else { 65 - max.leading_zeros() };
                let raw_cost = 5 + raw_bits as u64 * p.len() as u64;
                if raw_bits <= 31 && raw_cost < rice {
                    b.put(15, 4);
                    b.put(raw_bits as u64, 5);
                    for &v in p {
                        b.put(v as u64, raw_bits);
                    }
                } else {
                    b.put(k as u64, 4);
                    for &v in p {
                        let u = zigzag(v);
                        b.unary(u >> k);
                        b.put(u, k);
                    }
                }
            }
        }
    }
}

fn utf8_number(b: &mut Bits, v: u64) {
    if v < 0x80 {
        b.put(v, 8);
        return;
    }
    let mut extra = 1;
    while v >= 1u64 << (6 * extra + (6 - extra)) {
        extra += 1;
    }
    let lead_bits = 6 - extra; // data bits in the first byte
    let prefix = (0xFF00u64 >> (extra + 1)) & 0xFF;
    b.put(prefix | (v >> (6 * extra)) & ((1 << lead_bits) - 1), 8);
    for i in (0..extra).rev() {
        b.put(0x80 | ((v >> (6 * i)) & 0x3F), 8);
    }
}

fn sample_size_code(bps: u32) -> u64 {
    match bps {
        8 => 1,
        12 => 2,
        16 => 4,
        20 => 5,
        24 => 6,
        _ => 0,
    }
}

fn sample_rate_code(sr: u32) -> u64 {
    match sr {
        88_200 => 1,
        176_400 => 2,
        192_000 => 3,
        8_000 => 4,
        16_000 => 5,
        22_050 => 6,
        24_000 => 7,
        32_000 => 8,
        44_100 => 9,
        48_000 => 10,
        96_000 => 11,
        _ => 0,
    }
}

fn frame(out: &mut Vec<u8>, number: u64, l: &[i64], r: &[i64], bps: u32, sr: u32) {
    let n = l.len();
    let mid: Vec<i64> = l.iter().zip(r).map(|(a, b)| (a + b) >> 1).collect();
    let side: Vec<i64> = l.iter().zip(r).map(|(a, b)| a - b).collect();
    let (sl, bl) = analyze(l, bps);
    let (sr_, br) = analyze(r, bps);
    let (sm, bm) = analyze(&mid, bps);
    let (ss, bs) = analyze(&side, bps + 1);
    // (channel assignment, first, second)
    let options = [(0b0001u64, bl + br), (0b1000, bl + bs), (0b1001, bs + br), (0b1010, bm + bs)];
    let (mode, _) = options.iter().copied().min_by_key(|o| o.1).unwrap_or((1, 0));

    let mut b = Bits::new();
    b.put(0b11_1111_1111_1110, 14);
    b.put(0, 2); // reserved, fixed block size
    b.put(if n == BLOCK { 12 } else { 7 }, 4);
    b.put(sample_rate_code(sr), 4);
    b.put(mode, 4);
    b.put(sample_size_code(bps), 3);
    b.put(0, 1);
    utf8_number(&mut b, number);
    if n != BLOCK {
        b.put(n as u64 - 1, 16);
    }
    let c = crc8(&b.buf);
    b.put(c as u64, 8);
    match mode {
        0b0001 => {
            write_sub(&mut b, l, bps, &sl);
            write_sub(&mut b, r, bps, &sr_);
        }
        0b1000 => {
            write_sub(&mut b, l, bps, &sl);
            write_sub(&mut b, &side, bps + 1, &ss);
        }
        0b1001 => {
            write_sub(&mut b, &side, bps + 1, &ss);
            write_sub(&mut b, r, bps, &sr_);
        }
        _ => {
            write_sub(&mut b, &mid, bps, &sm);
            write_sub(&mut b, &side, bps + 1, &ss);
        }
    }
    b.align();
    let c = crc16(&b.buf);
    b.buf.extend_from_slice(&c.to_be_bytes());
    out.extend_from_slice(&b.buf);
}

/// Interleaved stereo samples (integers of `bps` bits) as a FLAC file.
pub fn encode(interleaved: &[i32], bps: u32, sample_rate: u32, title: &str) -> Vec<u8> {
    let total = interleaved.len() / 2;
    let bytes_per = bps.div_ceil(8) as usize;
    let mut md5 = Md5::new();
    let mut raw = Vec::with_capacity(BLOCK * 2 * bytes_per);
    let mut frames = Vec::new();
    let (mut min_frame, mut max_frame) = (u32::MAX, 0u32);
    for (number, chunk) in interleaved.chunks(BLOCK * 2).enumerate() {
        raw.clear();
        for &v in chunk {
            raw.extend_from_slice(&v.to_le_bytes()[..bytes_per]);
        }
        md5.update(&raw);
        let l: Vec<i64> = chunk.iter().step_by(2).map(|&v| v as i64).collect();
        let r: Vec<i64> = chunk.iter().skip(1).step_by(2).map(|&v| v as i64).collect();
        let before = frames.len();
        frame(&mut frames, number as u64, &l, &r, bps, sample_rate);
        let size = (frames.len() - before) as u32;
        min_frame = min_frame.min(size);
        max_frame = max_frame.max(size);
    }

    let mut out = b"fLaC".to_vec();
    let mut info = Bits::new();
    let block = if total < BLOCK { total.max(16) } else { BLOCK } as u64;
    info.put(block, 16);
    info.put(block, 16);
    info.put(if min_frame == u32::MAX { 0 } else { min_frame } as u64, 24);
    info.put(max_frame as u64, 24);
    info.put(sample_rate as u64, 20);
    info.put(1, 3); // two channels
    info.put(bps as u64 - 1, 5);
    info.put((total as u64) >> 32, 4);
    info.put(total as u64 & 0xFFFF_FFFF, 32);
    out.push(0); // STREAMINFO, not the last block
    out.extend_from_slice(&(34u32.to_be_bytes()[1..]));
    out.extend_from_slice(&info.buf);
    out.extend_from_slice(&md5.finish());

    let vendor = "Klangwelt";
    let comments = [format!("TITLE={title}"), "ENCODER=Klangwelt".to_string()];
    let mut vc = Vec::new();
    vc.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    vc.extend_from_slice(vendor.as_bytes());
    vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for c in &comments {
        vc.extend_from_slice(&(c.len() as u32).to_le_bytes());
        vc.extend_from_slice(c.as_bytes());
    }
    out.push(0x80 | 4); // VORBIS_COMMENT, last block
    out.extend_from_slice(&((vc.len() as u32).to_be_bytes()[1..]));
    out.extend_from_slice(&vc);
    out.extend_from_slice(&frames);
    out
}

// ---------------------------------------------------------------- Verification decoder

/// Small decoder for exactly what `encode` writes (for tests):
/// returns (sample rate, bits, interleaved samples) and checks all checksums.
#[cfg(test)]
pub fn decode(data: &[u8]) -> Result<(u32, u32, Vec<i32>), String> {
    struct R<'a> {
        d: &'a [u8],
        pos: usize, // in bits
    }
    impl R<'_> {
        fn get(&mut self, bits: u32) -> Result<u64, String> {
            let mut v = 0u64;
            for _ in 0..bits {
                let byte = *self.d.get(self.pos / 8).ok_or("Datei zu kurz")?;
                v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u64;
                self.pos += 1;
            }
            Ok(v)
        }
        fn signed(&mut self, bits: u32) -> Result<i64, String> {
            if bits == 0 {
                return Ok(0);
            }
            let v = self.get(bits)?;
            Ok(((v << (64 - bits)) as i64) >> (64 - bits))
        }
        fn unary(&mut self) -> Result<u64, String> {
            let mut q = 0;
            while self.get(1)? == 0 {
                q += 1;
            }
            Ok(q)
        }
        fn utf8(&mut self) -> Result<u64, String> {
            let first = self.get(8)?;
            let extra = (first as u8).leading_ones().saturating_sub(1);
            let mut v = first & (0x7F >> extra.max(if first < 0x80 { 0 } else { 1 }));
            for _ in 0..extra {
                v = (v << 6) | (self.get(8)? & 0x3F);
            }
            Ok(v)
        }
    }
    if !data.starts_with(b"fLaC") {
        return Err("keine FLAC-Kennung".into());
    }
    let mut pos = 4;
    let (mut sr, mut bps, mut total, mut md5_want) = (0u32, 0u32, 0u64, [0u8; 16]);
    loop {
        let head = data[pos];
        let len = u32::from_be_bytes([0, data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        if head & 0x7F == 0 {
            let mut r = R { d: &data[pos + 4..pos + 4 + len], pos: 0 };
            r.get(16 + 16 + 24 + 24)?;
            sr = r.get(20)? as u32;
            let ch = r.get(3)? + 1;
            bps = r.get(5)? as u32 + 1;
            total = r.get(36)?;
            if ch != 2 {
                return Err("nicht Stereo".into());
            }
            md5_want.copy_from_slice(&data[pos + 4 + 18..pos + 4 + 34]);
        }
        pos += 4 + len;
        if head & 0x80 != 0 {
            break;
        }
    }
    let mut out: Vec<i32> = Vec::new();
    while pos < data.len() {
        let mut r = R { d: &data[pos..], pos: 0 };
        if r.get(14)? != 0b11_1111_1111_1110 {
            return Err(format!("kein Frame bei Byte {pos}"));
        }
        r.get(2)?;
        let bs_code = r.get(4)?;
        r.get(4)?;
        let mode = r.get(4)?;
        r.get(4)?;
        r.utf8()?;
        let n = match bs_code {
            12 => BLOCK,
            7 => r.get(16)? as usize + 1,
            c => return Err(format!("Blockgröße {c} unerwartet")),
        };
        let head_len = r.pos / 8;
        if r.get(8)? as u8 != crc8(&data[pos..pos + head_len]) {
            return Err("CRC-8 falsch".into());
        }
        let mut chans: Vec<Vec<i64>> = Vec::new();
        for c in 0..2 {
            let side = matches!((mode, c), (0b1000, 1) | (0b1001, 0) | (0b1010, 1));
            let b = bps + side as u32;
            r.get(1)?;
            let t = r.get(6)?;
            r.get(1)?;
            let x = match t {
                0 => vec![r.signed(b)?; n],
                1 => (0..n).map(|_| r.signed(b)).collect::<Result<_, _>>()?,
                8..=12 => {
                    let order = (t - 8) as usize;
                    let mut x: Vec<i64> = (0..order).map(|_| r.signed(b)).collect::<Result<_, _>>()?;
                    if r.get(2)? != 0 {
                        return Err("Rice2 unerwartet".into());
                    }
                    let po = r.get(4)? as u32;
                    let mut res = Vec::with_capacity(n);
                    for j in 0..1usize << po {
                        let count = (n >> po) - if j == 0 { order } else { 0 };
                        let k = r.get(4)? as u32;
                        if k == 15 {
                            let raw = r.get(5)? as u32;
                            for _ in 0..count {
                                res.push(r.signed(raw)?);
                            }
                        } else {
                            for _ in 0..count {
                                let u = (r.unary()? << k) | r.get(k)?;
                                res.push(((u >> 1) as i64) ^ -((u & 1) as i64));
                            }
                        }
                    }
                    for e in res {
                        let i = x.len();
                        let p = match order {
                            0 => 0,
                            1 => x[i - 1],
                            2 => 2 * x[i - 1] - x[i - 2],
                            3 => 3 * x[i - 1] - 3 * x[i - 2] + x[i - 3],
                            _ => 4 * x[i - 1] - 6 * x[i - 2] + 4 * x[i - 3] - x[i - 4],
                        };
                        x.push(p + e);
                    }
                    x
                }
                other => return Err(format!("Subframe-Art {other} unerwartet")),
            };
            chans.push(x);
        }
        r.pos = r.pos.div_ceil(8) * 8;
        let body = r.pos / 8;
        let want = u16::from_be_bytes([data[pos + body], data[pos + body + 1]]);
        if crc16(&data[pos..pos + body]) != want {
            return Err("CRC-16 falsch".into());
        }
        pos += body + 2;
        for i in 0..n {
            let (a, b) = (chans[0][i], chans[1][i]);
            let (l, rr) = match mode {
                0b0001 => (a, b),
                0b1000 => (a, a - b),
                0b1001 => (a + b, b),
                _ => {
                    let m = (a << 1) | (b & 1);
                    ((m + b) >> 1, (m - b) >> 1)
                }
            };
            out.push(l as i32);
            out.push(rr as i32);
        }
    }
    if out.len() as u64 != total * 2 {
        return Err(format!("{} statt {} Frames", out.len() / 2, total));
    }
    let mut md5 = Md5::new();
    let bytes_per = bps.div_ceil(8) as usize;
    for v in &out {
        md5.update(&v.to_le_bytes()[..bytes_per]);
    }
    if md5.finish() != md5_want {
        return Err("MD5 falsch".into());
    }
    Ok((sr, bps, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: [u8; 16]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_matches_reference_values() {
        let md5 = |s: &[u8]| {
            let mut m = Md5::new();
            m.update(s);
            hex(m.finish())
        };
        assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
        // fed in pieces = in one go
        let long: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let mut m = Md5::new();
        for c in long.chunks(37) {
            m.update(c);
        }
        assert_eq!(hex(m.finish()), md5(&long));
    }

    fn signal(n: usize, bps: u32) -> Vec<i32> {
        let amp = ((1i64 << (bps - 1)) - 1) as f64 * 0.7;
        let mut seed = 1u32;
        (0..n)
            .flat_map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 16) as f64 / 65536.0 - 0.5;
                let t = i as f64 / 48_000.0;
                let l = amp * (0.6 * (2.0 * std::f64::consts::PI * 220.0 * t).sin() + 0.02 * noise);
                let r = amp * (0.5 * (2.0 * std::f64::consts::PI * 330.0 * t).sin() + 0.02 * noise);
                [l.round() as i32, r.round() as i32]
            })
            .collect()
    }

    #[test]
    fn roundtrip_is_bit_exact_and_smaller() {
        for bps in [16, 24] {
            let n = 3 * BLOCK + 1234; // last block shorter
            let pcm = signal(n, bps);
            let file = encode(&pcm, bps, 48_000, "Test");
            let (sr, b, back) = decode(&file).unwrap();
            assert_eq!((sr, b), (48_000, bps));
            assert_eq!(back, pcm, "{bps} Bit verlustfrei");
            let raw = n * 2 * (bps as usize / 8);
            assert!(file.len() < raw * 3 / 4, "{bps} Bit: {} von {raw} Bytes", file.len());
        }
    }

    #[test]
    fn silence_extremes_and_tiny_files() {
        // silence (constant), full scale with jumps, a short remainder
        let mut pcm = vec![0i32; 2 * BLOCK];
        pcm.extend((0..BLOCK * 2).map(|i| if (i / 2) % 3 == 0 { 8_388_607 } else { -8_388_608 }));
        pcm.extend([5, -5, 7, -7]);
        let file = encode(&pcm, 24, 48_000, "Grenzen");
        assert_eq!(decode(&file).unwrap().2, pcm);
        let tiny = vec![1, 2, 3, 4];
        assert_eq!(decode(&encode(&tiny, 16, 48_000, "")).unwrap().2, tiny);
    }

    #[test]
    fn frame_numbers_beyond_one_byte() {
        let mut b = Bits::new();
        utf8_number(&mut b, 0x7F);
        utf8_number(&mut b, 0x80);
        utf8_number(&mut b, 0x7FF);
        utf8_number(&mut b, 0x800);
        assert_eq!(b.buf, [0x7F, 0xC2, 0x80, 0xDF, 0xBF, 0xE0, 0xA0, 0x80]);
    }
}
