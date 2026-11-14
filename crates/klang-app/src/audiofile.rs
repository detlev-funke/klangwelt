//! Reads audio files with its own decoders (only Opus goes through libopus, which
//! Klangwelt bundles for export anyway):
//!
//! * **WAV**: PCM with 8, 16, 24 or 32 bits, floating point with 32 or 64 bits,
//!   also as WAVE_FORMAT_EXTENSIBLE (how many programs store 24 bit).
//! * **AIFF/AIFC**: PCM (big endian), "sowt" (little endian), fl32/fl64.
//! * **FLAC**: complete - constant, verbatim, fixed and LPC subframes,
//!   Rice partitions, all stereo modes, arbitrary block sizes.
//! * **Ogg Opus**: up to two channels.
//!
//! Klangwelt keeps audio mono; multichannel material is averaged.
//! MP3, AAC/M4A, Ogg Vorbis and WMA are recognized at the start of the file and rejected with a
//! clear message (there is no decoder for them yet).

use klang_i18n::tr;

/// For messages: what can be opened (spelled out in the messages below).
pub const SUPPORTED: &str = "WAV, AIFF, FLAC und Opus";

/// Decoded audio (mono).
#[derive(Debug)]
pub struct Decoded {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Channels of the file (before averaging).
    pub channels: u16,
    /// Format in words, e.g. "WAV 24 bit".
    pub format: String,
}

impl Decoded {
    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / self.sample_rate.max(1) as f64
    }
}

/// `what`: the format (a name like "MP3" or an already translated description).
fn unsupported(what: &str) -> String {
    tr!(
        "Klangwelt can't read {what} yet – supported formats are WAV, AIFF, FLAC and Opus. Tip: save the file as WAV or FLAC in another program.",
        what = what
    )
}

/// Detects the format from the content (not the extension) and decodes.
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    let b = bytes;
    if b.len() < 12 {
        return Err(tr!("The file is empty or too short."));
    }
    if &b[..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        return wav(b);
    }
    if &b[..4] == b"RF64" {
        return Err(unsupported(&tr!("WAV in RF64 format (over 4 GB)")));
    }
    if &b[..4] == b"FORM" && (&b[8..12] == b"AIFF" || &b[8..12] == b"AIFC") {
        return aiff(b);
    }
    if &b[..4] == b"fLaC" {
        return flac(b);
    }
    if &b[..3] == b"ID3" {
        // ID3v2 tag: comes before MP3, sometimes before FLAC too
        let size = ((b[6] as usize & 0x7f) << 21) | ((b[7] as usize & 0x7f) << 14) | ((b[8] as usize & 0x7f) << 7) | (b[9] as usize & 0x7f);
        let skip = 10 + size + if b[5] & 0x10 != 0 { 10 } else { 0 };
        return match b.get(skip..) {
            Some(rest) if rest.starts_with(b"fLaC") => flac(rest),
            _ => Err(unsupported("MP3")),
        };
    }
    if &b[..4] == b"OggS" {
        return ogg(b);
    }
    if &b[4..8] == b"ftyp" {
        return Err(unsupported("AAC/M4A (MP4-Audio)"));
    }
    if b[..8] == [0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11] {
        return Err(unsupported("WMA"));
    }
    if b[0] == 0xFF && b[1] & 0xE0 == 0xE0 {
        // Frame sync word: layer bits 00 = AAC (ADTS), otherwise MPEG audio (MP3)
        return Err(unsupported(if b[1] & 0x06 == 0 { "AAC (ADTS)" } else { "MP3" }));
    }
    Err(tr!("Unknown file format – Klangwelt reads WAV, AIFF, FLAC and Opus."))
}

fn mono(frames: impl Iterator<Item = f32>, channels: usize) -> Vec<f32> {
    let ch = channels.max(1);
    if ch == 1 {
        return frames.collect();
    }
    let all: Vec<f32> = frames.collect();
    all.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect()
}

// ---------------------------------------------------------------- WAV

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// PCM samples (integer or floating point) → f32; `big` = big endian.
fn pcm(data: &[u8], bytes: usize, float: bool, big: bool, unsigned8: bool) -> Result<Vec<f32>, String> {
    let get = |c: &[u8]| -> f32 {
        let mut a = [0u8; 8];
        for (k, v) in c.iter().enumerate() {
            a[if big { bytes - 1 - k } else { k }] = *v;
        }
        match (float, bytes) {
            (true, 4) => f32::from_le_bytes([a[0], a[1], a[2], a[3]]),
            (true, _) => f64::from_le_bytes(a) as f32,
            (false, 1) if unsigned8 => (a[0] as f32 - 128.0) / 128.0,
            (false, 1) => a[0] as i8 as f32 / 128.0,
            (false, 2) => i16::from_le_bytes([a[0], a[1]]) as f32 / 32_768.0,
            (false, 3) => (i32::from_le_bytes([0, a[0], a[1], a[2]]) >> 8) as f32 / 8_388_608.0,
            (false, _) => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f32 / 2_147_483_648.0,
        }
    };
    if !matches!((float, bytes), (true, 4) | (true, 8) | (false, 1..=4)) {
        return Err(tr!("Sample format not supported"));
    }
    Ok(data.chunks_exact(bytes).map(get).collect())
}

fn wav(b: &[u8]) -> Result<Decoded, String> {
    let (mut i, mut fmt, mut data) = (12usize, None, None);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32le(b, i + 4) as usize;
        // tolerate truncated or indeterminate-length data chunks (recording programs)
        let body = &b[i + 8..(i + 8).saturating_add(len).min(b.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let mut tag = u16le(body, 0);
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = u16le(body, 24); // subformat (GUID starts with the format code)
                }
                fmt = Some((tag, u16le(body, 2).max(1), u32le(body, 4), u16le(body, 12), u16le(body, 14)));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        if data.is_some() && fmt.is_some() {
            break;
        }
        i = match (i + 8).checked_add(len + (len & 1)) {
            Some(n) => n,
            None => break,
        };
    }
    let (tag, channels, rate, align, bits) = fmt.ok_or_else(|| tr!("WAV: format chunk (fmt) missing"))?;
    let data = data.ok_or_else(|| tr!("WAV: no audio data (data)"))?;
    let container = (align as usize / channels as usize).max(1);
    let float = match tag {
        1 => false,
        3 => true,
        0x55 => return Err(unsupported(&tr!("MP3 in a WAV file"))),
        2 | 0x11 => return Err(unsupported(&tr!("ADPCM-compressed WAV"))),
        6 | 7 => return Err(unsupported("A-law/µ-law-WAV")),
        t => return Err(tr!("WAV encoding {code} is not supported (only PCM and floating point).", code = format!("{t:#x}"))),
    };
    if rate == 0 {
        return Err(tr!("WAV: sample rate 0"));
    }
    let frames = pcm(data, container, float, false, true).map_err(|e| tr!("WAV: {error} ({bits}-bit)", error = e, bits = bits))?;
    let format = if float { tr!("WAV {bits}-bit float", bits = bits) } else { tr!("WAV {bits}-bit", bits = bits) };
    Ok(Decoded { samples: mono(frames.into_iter(), channels as usize), sample_rate: rate, channels, format })
}

// ---------------------------------------------------------------- AIFF

fn u32be(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// 80-bit floating-point number (IEEE extended) of the sample rate.
fn extended(b: &[u8]) -> f64 {
    let exp = (((b[0] as i32) & 0x7f) << 8) | b[1] as i32;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    let v = mant as f64 * 2f64.powi(exp - 16383 - 63);
    if b[0] & 0x80 != 0 { -v } else { v }
}

fn aiff(b: &[u8]) -> Result<Decoded, String> {
    let aifc = &b[8..12] == b"AIFC";
    let (mut i, mut comm, mut ssnd) = (12usize, None, None);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32be(b, i + 4) as usize;
        let body = &b[i + 8..(i + 8).saturating_add(len).min(b.len())];
        match id {
            b"COMM" if body.len() >= 18 => {
                let channels = u16::from_be_bytes([body[0], body[1]]).max(1);
                let bits = u16::from_be_bytes([body[6], body[7]]);
                let rate = extended(&body[8..18]);
                let comp = if aifc && body.len() >= 22 { <[u8; 4]>::try_from(&body[18..22]).unwrap() } else { *b"NONE" };
                comm = Some((channels, bits, rate, comp));
            }
            b"SSND" if body.len() >= 8 => {
                let offset = u32be(body, 0) as usize;
                ssnd = body.get(8 + offset..);
            }
            _ => {}
        }
        i = match (i + 8).checked_add(len + (len & 1)) {
            Some(n) => n,
            None => break,
        };
    }
    let (channels, bits, rate, comp) = comm.ok_or_else(|| tr!("AIFF: format chunk (COMM) missing"))?;
    let data = ssnd.ok_or_else(|| tr!("AIFF: no audio data (SSND)"))?;
    let bytes = (bits as usize).div_ceil(8).max(1);
    let frames = match &comp {
        b"NONE" | b"twos" => pcm(data, bytes, false, true, false),
        b"sowt" => pcm(data, bytes, false, false, false),
        b"raw " => pcm(data, 1, false, false, true),
        b"fl32" | b"FL32" => pcm(data, 4, true, true, false),
        b"fl64" | b"FL64" => pcm(data, 8, true, true, false),
        other => return Err(unsupported(&tr!("AIFC with \"{name}\" compression", name = String::from_utf8_lossy(other)))),
    }
    .map_err(|e| format!("AIFF: {e}"))?;
    if !(rate.is_finite() && rate >= 1.0) {
        return Err(tr!("AIFF: invalid sample rate"));
    }
    let format = tr!("AIFF {bits}-bit", bits = bits);
    Ok(Decoded { samples: mono(frames.into_iter(), channels as usize), sample_rate: rate.round() as u32, channels, format })
}

// ---------------------------------------------------------------- FLAC

struct BitReader<'a> {
    d: &'a [u8],
    /// Position in bits.
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(d: &'a [u8]) -> Self {
        BitReader { d, pos: 0 }
    }

    /// Eight bytes starting at `byte` (padded with zeros past the end).
    fn window(&self, byte: usize) -> u64 {
        if byte + 8 <= self.d.len() {
            return u64::from_be_bytes(self.d[byte..byte + 8].try_into().unwrap());
        }
        let mut a = [0u8; 8];
        for (k, v) in a.iter_mut().enumerate() {
            *v = self.d.get(byte + k).copied().unwrap_or(0);
        }
        u64::from_be_bytes(a)
    }

    /// `n` bits (at most 57), unsigned.
    fn read(&mut self, n: u32) -> Result<u64, String> {
        if n == 0 {
            return Ok(0);
        }
        if self.pos + n as usize > self.d.len() * 8 {
            return Err(tr!("File ends in the middle of a block"));
        }
        let w = self.window(self.pos / 8) << (self.pos % 8);
        self.pos += n as usize;
        Ok(w >> (64 - n))
    }

    fn signed(&mut self, n: u32) -> Result<i64, String> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.read(n)?;
        Ok(((v << (64 - n)) as i64) >> (64 - n))
    }

    /// Counts zeros up to the next one (the one is consumed).
    fn unary(&mut self) -> Result<u32, String> {
        let mut q = 0u32;
        loop {
            let byte = self.pos / 8;
            if byte >= self.d.len() {
                return Err(tr!("File ends in the middle of a block"));
            }
            let off = (self.pos % 8) as u32;
            let w = self.window(byte) << off;
            let avail = 64 - off;
            let lz = w.leading_zeros();
            if lz < avail {
                self.pos += lz as usize + 1;
                return Ok(q + lz);
            }
            q += avail;
            self.pos += avail as usize;
        }
    }

    fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    fn byte_pos(&self) -> usize {
        self.pos / 8
    }
}

struct StreamInfo {
    rate: u32,
    channels: usize,
    bps: u32,
}

fn flac(b: &[u8]) -> Result<Decoded, String> {
    let mut i = 4usize;
    let mut info = None;
    loop {
        if i + 4 > b.len() {
            return Err(tr!("FLAC: metadata truncated"));
        }
        let last = b[i] & 0x80 != 0;
        let kind = b[i] & 0x7f;
        let len = ((b[i + 1] as usize) << 16) | ((b[i + 2] as usize) << 8) | b[i + 3] as usize;
        let body = b.get(i + 4..i + 4 + len).ok_or_else(|| tr!("FLAC: metadata truncated"))?;
        if kind == 0 && len >= 18 {
            let rate = ((body[10] as u32) << 12) | ((body[11] as u32) << 4) | (body[12] as u32 >> 4);
            let channels = ((body[12] >> 1) & 0x07) as usize + 1;
            let bps = ((((body[12] & 1) as u32) << 4) | (body[13] as u32 >> 4)) + 1;
            info = Some(StreamInfo { rate, channels, bps });
        }
        i += 4 + len;
        if last {
            break;
        }
    }
    let info = info.ok_or_else(|| tr!("FLAC: STREAMINFO missing"))?;
    let mut out: Vec<f32> = Vec::new();
    let mut r = BitReader::new(&b[i..]);
    let mut bps_seen = info.bps;
    let mut rate = info.rate;
    while r.byte_pos() + 2 <= r.d.len() {
        let start = r.pos;
        match flac_frame(&mut r, &info, &mut out) {
            Ok((fr_rate, fr_bps)) => {
                if fr_rate > 0 {
                    rate = fr_rate;
                }
                bps_seen = fr_bps;
            }
            Err(e) => {
                // corrupt block: continue to the next sync word (rest of a file with tags at the end, etc.)
                let mut k = start / 8 + 1;
                let d = r.d;
                while k + 1 < d.len() && !(d[k] == 0xFF && d[k + 1] & 0xFE == 0xF8) {
                    k += 1;
                }
                if k + 1 >= d.len() {
                    if out.is_empty() {
                        return Err(format!("FLAC: {e}"));
                    }
                    break;
                }
                r.pos = k * 8;
            }
        }
    }
    if out.is_empty() {
        return Err(tr!("FLAC: no audio data"));
    }
    if rate == 0 {
        return Err(tr!("FLAC: unknown sample rate"));
    }
    let format = tr!("FLAC {bits}-bit", bits = bps_seen);
    Ok(Decoded { samples: out, sample_rate: rate, channels: info.channels as u16, format })
}

/// Appends one FLAC block to `out` as mono; returns (sample rate, bits) of the block.
fn flac_frame(r: &mut BitReader, info: &StreamInfo, out: &mut Vec<f32>) -> Result<(u32, u32), String> {
    if r.read(14)? != 0x3FFE {
        return Err(tr!("Sync code missing"));
    }
    r.read(2)?; // reserved, block type
    let bs_code = r.read(4)?;
    let sr_code = r.read(4)?;
    let ch_code = r.read(4)? as u32;
    let ss_code = r.read(3)?;
    r.read(1)?;
    // block/sample number (UTF-8-like coding)
    let first = r.read(8)?;
    let extra = match first {
        0x00..=0x7F => 0,
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        0xF8..=0xFB => 4,
        0xFC..=0xFD => 5,
        0xFE => 6,
        _ => return Err(tr!("invalid block number")),
    };
    for _ in 0..extra {
        r.read(8)?;
    }
    let block = match bs_code {
        0 => return Err(tr!("reserved block size")),
        1 => 192,
        2..=5 => 576 << (bs_code - 2),
        6 => r.read(8)? as usize + 1,
        7 => r.read(16)? as usize + 1,
        _ => 256 << (bs_code - 8),
    };
    let rate = match sr_code {
        0 => info.rate,
        1 => 88_200,
        2 => 176_400,
        3 => 192_000,
        4 => 8_000,
        5 => 16_000,
        6 => 22_050,
        7 => 24_000,
        8 => 32_000,
        9 => 44_100,
        10 => 48_000,
        11 => 96_000,
        12 => r.read(8)? as u32 * 1000,
        13 => r.read(16)? as u32,
        14 => r.read(16)? as u32 * 10,
        _ => return Err(tr!("invalid sample rate")),
    };
    let bps = match ss_code {
        0 => info.bps,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return Err(tr!("invalid bit depth")),
    };
    r.read(8)?; // CRC-8 of the header
    let channels = match ch_code {
        0..=7 => ch_code as usize + 1,
        8..=10 => 2,
        _ => return Err(tr!("invalid channel assignment")),
    };
    let mut chans: Vec<Vec<i64>> = Vec::with_capacity(channels);
    for c in 0..channels {
        // side channel (difference) needs one more bit
        let side = matches!((ch_code, c), (8, 1) | (9, 0) | (10, 1));
        chans.push(flac_subframe(r, block, bps + side as u32)?);
    }
    r.align();
    r.read(16)?; // CRC-16 of the block
    match ch_code {
        8 => {
            let (l, s) = (&chans[0], &chans[1]);
            chans[1] = l.iter().zip(s).map(|(l, s)| l - s).collect();
        }
        9 => {
            let (s, rr) = (&chans[0], &chans[1]);
            chans[0] = s.iter().zip(rr).map(|(s, r)| s + r).collect();
        }
        10 => {
            let (m, s) = (chans[0].clone(), chans[1].clone());
            for k in 0..block {
                let mid = (m[k] << 1) | (s[k] & 1);
                chans[0][k] = (mid + s[k]) >> 1;
                chans[1][k] = (mid - s[k]) >> 1;
            }
        }
        _ => {}
    }
    let scale = 1.0 / (channels as f64 * (1u64 << (bps - 1)) as f64);
    out.reserve(block);
    for k in 0..block {
        let sum: i64 = chans.iter().map(|c| c[k]).sum();
        out.push((sum as f64 * scale) as f32);
    }
    Ok((rate, bps))
}

fn flac_subframe(r: &mut BitReader, block: usize, bps: u32) -> Result<Vec<i64>, String> {
    if r.read(1)? != 0 {
        return Err(tr!("Subframe: padding bit set"));
    }
    let kind = r.read(6)? as u32;
    let wasted = if r.read(1)? == 1 { r.unary()? + 1 } else { 0 };
    let bps = bps.checked_sub(wasted).filter(|&b| b > 0).ok_or_else(|| tr!("Subframe: too many wasted bits"))?;
    let mut s: Vec<i64> = match kind {
        0 => vec![r.signed(bps)?; block],
        1 => (0..block).map(|_| r.signed(bps)).collect::<Result<_, _>>()?,
        8..=12 => {
            let order = (kind - 8) as usize;
            if order > block {
                return Err(tr!("Subframe: order larger than block"));
            }
            let mut s: Vec<i64> = (0..order).map(|_| r.signed(bps)).collect::<Result<_, _>>()?;
            let res = flac_residual(r, block, order)?;
            s.reserve(block);
            for (i, e) in res.into_iter().enumerate() {
                let n = i + order;
                let p = match order {
                    0 => 0,
                    1 => s[n - 1],
                    2 => 2 * s[n - 1] - s[n - 2],
                    3 => 3 * s[n - 1] - 3 * s[n - 2] + s[n - 3],
                    _ => 4 * s[n - 1] - 6 * s[n - 2] + 4 * s[n - 3] - s[n - 4],
                };
                s.push(p + e);
            }
            s
        }
        32..=63 => {
            let order = (kind & 31) as usize + 1;
            if order > block {
                return Err(tr!("Subframe: order larger than block"));
            }
            let mut s: Vec<i64> = (0..order).map(|_| r.signed(bps)).collect::<Result<_, _>>()?;
            let precision = r.read(4)? as u32 + 1;
            if precision == 16 {
                return Err(tr!("Subframe: invalid coefficient precision"));
            }
            let shift = r.signed(5)?;
            if shift < 0 {
                return Err(tr!("Subframe: negative shift"));
            }
            let coef: Vec<i64> = (0..order).map(|_| r.signed(precision)).collect::<Result<_, _>>()?;
            let res = flac_residual(r, block, order)?;
            s.reserve(block);
            for (i, e) in res.into_iter().enumerate() {
                let n = i + order;
                let p: i64 = coef.iter().enumerate().map(|(j, c)| c * s[n - 1 - j]).sum();
                s.push((p >> shift) + e);
            }
            s
        }
        _ => return Err(tr!("Subframe: unknown type {kind}", kind = kind)),
    };
    if wasted > 0 {
        s.iter_mut().for_each(|v| *v <<= wasted);
    }
    Ok(s)
}

fn flac_residual(r: &mut BitReader, block: usize, order: usize) -> Result<Vec<i64>, String> {
    let (param_bits, escape) = match r.read(2)? {
        0 => (4, 15),
        1 => (5, 31),
        _ => return Err(tr!("Residual: unknown coding")),
    };
    let part_order = r.read(4)? as u32;
    let parts = 1usize << part_order;
    if block % parts != 0 || (block >> part_order) < order {
        return Err(tr!("Residual: invalid partitions"));
    }
    let mut out = Vec::with_capacity(block - order);
    for p in 0..parts {
        let n = (block >> part_order) - if p == 0 { order } else { 0 };
        let k = r.read(param_bits)? as u32;
        if k == escape {
            let bits = r.read(5)? as u32;
            for _ in 0..n {
                out.push(r.signed(bits)?);
            }
        } else {
            for _ in 0..n {
                let q = r.unary()? as u64;
                let v = (q << k) | r.read(k)?;
                out.push(((v >> 1) as i64) ^ -((v & 1) as i64));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- Ogg

/// Packets of the first logical stream of an Ogg file and the last granule position.
fn ogg_packets(b: &[u8]) -> Result<(Vec<Vec<u8>>, u64), String> {
    let (mut pos, mut packets, mut cur, mut serial, mut granule) = (0usize, Vec::new(), Vec::new(), None, 0u64);
    while pos + 27 <= b.len() {
        if &b[pos..pos + 4] != b"OggS" {
            return Err(tr!("Ogg: page at byte {pos} is corrupted", pos = pos));
        }
        let gp = u64::from_le_bytes(b[pos + 6..pos + 14].try_into().unwrap());
        let ser = u32le(b, pos + 14);
        let nseg = b[pos + 26] as usize;
        let Some(lacing) = b.get(pos + 27..pos + 27 + nseg) else { break };
        let body_len: usize = lacing.iter().map(|&l| l as usize).sum();
        let mut off = pos + 27 + nseg;
        if off + body_len > b.len() {
            break; // truncated last page
        }
        let mine = *serial.get_or_insert(ser) == ser;
        if mine {
            for &l in lacing {
                cur.extend_from_slice(&b[off..off + l as usize]);
                off += l as usize;
                if l < 255 {
                    packets.push(std::mem::take(&mut cur));
                }
            }
            if gp != u64::MAX {
                granule = gp;
            }
        }
        pos += 27 + nseg + body_len;
    }
    Ok((packets, granule))
}

fn ogg(b: &[u8]) -> Result<Decoded, String> {
    let (packets, granule) = ogg_packets(b)?;
    let head = packets.first().ok_or_else(|| tr!("Ogg: empty"))?;
    if head.starts_with(b"\x01vorbis") {
        return Err(unsupported("Ogg Vorbis"));
    }
    if head.starts_with(b"\x7fFLAC") {
        return Err(unsupported(&tr!("FLAC in an Ogg container")));
    }
    if !head.starts_with(b"OpusHead") || head.len() < 19 {
        return Err(unsupported(&tr!("this Ogg format")));
    }
    let channels = head[9] as u16;
    if channels > 2 {
        return Err(unsupported(&tr!("Opus with more than two channels")));
    }
    let pre_skip = u16le(head, 10) as usize;
    let gain_db = i16::from_le_bytes([head[16], head[17]]) as f32 / 256.0;
    let gain = 10f32.powf(gain_db / 20.0);
    let mut dec = klang_opus::Decoder::new()?;
    let mut stereo: Vec<f32> = Vec::new();
    for p in packets.iter().skip(2) {
        if p.is_empty() {
            continue;
        }
        stereo.extend(dec.decode(p).map_err(|e| format!("Opus: {e}"))?);
    }
    let mut frames: Vec<f32> = stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5 * gain).collect();
    let skip = pre_skip.min(frames.len());
    frames.drain(..skip);
    let total = (granule as usize).saturating_sub(pre_skip);
    if total > 0 && total < frames.len() {
        frames.truncate(total);
    }
    if frames.is_empty() {
        return Err(tr!("Opus: no audio data"));
    }
    Ok(Decoded { samples: frames, sample_rate: 48_000, channels: channels.max(1), format: "Opus".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, sr: f32) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr).sin() * 0.5).collect()
    }

    fn riff(fmt: &[u8], data: &[u8]) -> Vec<u8> {
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        b.extend_from_slice(fmt);
        b.extend_from_slice(b"LIST\x04\0\0\0abcd"); // foreign chunk in between
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(data);
        b
    }

    fn fmt(tag: u16, ch: u16, rate: u32, bits: u16) -> Vec<u8> {
        let align = ch * bits / 8;
        let mut f = Vec::new();
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&ch.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * align as u32).to_le_bytes());
        f.extend_from_slice(&align.to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f
    }

    #[test]
    fn wav_bit_depths_and_extensible() {
        let x = sine(400, 8000.0);
        // 24-bit stereo as WAVE_FORMAT_EXTENSIBLE (right silent → mono = half the volume)
        let mut f = fmt(0xFFFE, 2, 8000, 24);
        f.extend_from_slice(&22u16.to_le_bytes());
        f.extend_from_slice(&24u16.to_le_bytes());
        f.extend_from_slice(&3u32.to_le_bytes());
        f.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
        let mut data = Vec::new();
        for v in &x {
            let i = (v * 8_388_607.0) as i32;
            data.extend_from_slice(&i.to_le_bytes()[..3]);
            data.extend_from_slice(&[0, 0, 0]);
        }
        let d = decode(&riff(&f, &data)).unwrap();
        assert_eq!((d.sample_rate, d.channels, d.samples.len()), (8000, 2, 400));
        assert!(d.samples.iter().zip(&x).all(|(a, b)| (a - b / 2.0).abs() < 1e-5));
        assert_eq!(d.format, "WAV 24-bit");
        // 8 bit (unsigned), 32-bit PCM, 64-bit floating point
        let d8 = decode(&riff(&fmt(1, 1, 8000, 8), &x.iter().map(|v| (v * 127.0 + 128.0) as u8).collect::<Vec<_>>())).unwrap();
        assert!(d8.samples.iter().zip(&x).all(|(a, b)| (a - b).abs() < 0.02));
        let d32: Vec<u8> = x.iter().flat_map(|v| ((v * 2_147_483_000.0) as i32).to_le_bytes()).collect();
        assert!(decode(&riff(&fmt(1, 1, 8000, 32), &d32)).unwrap().samples.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-6));
        let d64: Vec<u8> = x.iter().flat_map(|v| (*v as f64).to_le_bytes()).collect();
        assert!(decode(&riff(&fmt(3, 1, 8000, 64), &d64)).unwrap().samples.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-7));
    }

    #[test]
    fn aiff_big_endian_and_sowt() {
        let x = sine(300, 22_050.0);
        let build = |kind: &[u8; 4], comp: Option<&[u8; 4]>, little: bool| {
            let mut comm = Vec::new();
            comm.extend_from_slice(&1u16.to_be_bytes());
            comm.extend_from_slice(&(x.len() as u32).to_be_bytes());
            comm.extend_from_slice(&16u16.to_be_bytes());
            // 22050 as an 80-bit number: exponent 16383+14, mantissa 22050 << 49
            comm.extend_from_slice(&((16383u16 + 14).to_be_bytes()));
            comm.extend_from_slice(&((22_050u64) << 49).to_be_bytes());
            if let Some(c) = comp {
                comm.extend_from_slice(c);
                comm.extend_from_slice(b"\0\0");
            }
            let mut ssnd = vec![0u8; 8];
            for v in &x {
                let s = (v * 32767.0) as i16;
                ssnd.extend_from_slice(&if little { s.to_le_bytes() } else { s.to_be_bytes() });
            }
            let mut b = b"FORM\0\0\0\0".to_vec();
            b.extend_from_slice(kind);
            for (id, body) in [(b"COMM", &comm), (b"SSND", &ssnd)] {
                b.extend_from_slice(id);
                b.extend_from_slice(&(body.len() as u32).to_be_bytes());
                b.extend_from_slice(body);
            }
            b
        };
        for file in [build(b"AIFF", None, false), build(b"AIFC", Some(b"sowt"), true)] {
            let d = decode(&file).unwrap();
            assert_eq!((d.sample_rate, d.samples.len()), (22_050, 300));
            assert!(d.samples.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-3));
        }
    }

    #[test]
    fn flac_from_the_own_encoder_with_all_stereo_modes() {
        // stereo with different channels (forces side-channel modes), 16 bit, several blocks
        let n = 10_000;
        let l: Vec<i32> = (0..n).map(|i| ((i as f32 * 0.03).sin() * 12_000.0) as i32).collect();
        let r: Vec<i32> = (0..n).map(|i| ((i as f32 * 0.031).sin() * 11_000.0 + (i % 7) as f32 * 30.0) as i32).collect();
        let inter: Vec<i32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
        let file = crate::flac::encode(&inter, 16, 44_100, "Test");
        let d = decode(&file).unwrap();
        assert_eq!((d.sample_rate, d.channels, d.samples.len()), (44_100, 2, n));
        for k in 0..n {
            let want = (l[k] + r[k]) as f32 / 2.0 / 32_768.0;
            assert!((d.samples[k] - want).abs() < 1e-6, "Sample {k}");
        }
        assert_eq!(d.format, "FLAC 16-bit");
    }

    /// Small bit writer for hand-built FLAC blocks.
    struct W(Vec<u8>, u32);
    impl W {
        fn put(&mut self, v: u64, n: u32) {
            for k in (0..n).rev() {
                if self.1 % 8 == 0 {
                    self.0.push(0);
                }
                let bit = ((v >> k) & 1) as u8;
                let last = self.0.len() - 1;
                self.0[last] |= bit << (7 - self.1 % 8);
                self.1 += 1;
            }
        }
        fn rice(&mut self, v: i64, k: u32) {
            let z = ((v << 1) ^ (v >> 63)) as u64;
            for _ in 0..(z >> k) {
                self.put(0, 1);
            }
            self.put(1, 1);
            self.put(z & ((1 << k) - 1), k);
        }
    }

    #[test]
    fn flac_lpc_subframe_and_wasted_bits() {
        // mono, 16 bit, block 32: 2nd-order LPC (2·s[n-1] − s[n-2], precision 4, shift 0)
        let sig: Vec<i64> = (0..32).map(|i| 100 + 7 * i + (i * i) % 5).collect();
        let mut w = W(Vec::new(), 0);
        // STREAMINFO
        let mut b = b"fLaC".to_vec();
        b.extend_from_slice(&[0x80, 0, 0, 34]);
        let mut si = vec![0u8; 34];
        si[0..2].copy_from_slice(&32u16.to_be_bytes());
        si[2..4].copy_from_slice(&32u16.to_be_bytes());
        // rate 16000 (20 bits), 1 channel (0), 16 bit (15), samples 64
        let packed: u64 = (16_000u64 << 44) | (0 << 41) | (15 << 36) | 64;
        si[10..18].copy_from_slice(&packed.to_be_bytes());
        b.extend_from_slice(&si);
        for frame in 0..2u64 {
            w.put(0x3FFE, 14);
            w.put(0, 2);
            w.put(7, 4); // block size: 16 bits follow
            w.put(0, 4); // rate from STREAMINFO
            w.put(0, 4); // 1 channel
            w.put(4, 3); // 16 bit
            w.put(0, 1);
            w.put(frame, 8);
            w.put(31, 16); // 32 − 1
            w.put(0, 8); // CRC-8 (not checked)
            if frame == 0 {
                // LPC subframe
                w.put(0, 1);
                w.put(32 + 1, 6); // LPC, order 2
                w.put(0, 1);
                w.put(sig[0] as u64, 16);
                w.put(sig[1] as u64, 16);
                w.put(3, 4); // precision 4
                w.put(0, 5); // shift 0
                w.put(2, 4); // +2
                w.put(0b1111, 4); // −1
                w.put(0, 2);
                w.put(0, 4); // one partition
                w.put(2, 4); // Rice k = 2
                for n in 2..32 {
                    w.rice(sig[n] - (2 * sig[n - 1] - sig[n - 2]), 2);
                }
            } else {
                // constant with one unused bit: 2 × (−3) = −6
                w.put(0, 1);
                w.put(0, 6);
                w.put(1, 1);
                w.put(1, 1); // unary: 0 zeros → 1 unused bit
                w.put((-3i64 as u64) & 0x7fff, 15);
            }
            let pad = (8 - w.1 % 8) % 8;
            w.put(0, pad);
            w.put(0, 16); // CRC-16 (not checked)
        }
        b.extend_from_slice(&w.0);
        let d = decode(&b).unwrap();
        assert_eq!(d.samples.len(), 64);
        for (k, v) in sig.iter().enumerate() {
            assert!((d.samples[k] * 32_768.0 - *v as f32).abs() < 1e-3, "Sample {k}: {}", d.samples[k] * 32_768.0);
        }
        assert!((d.samples[40] * 32_768.0 + 6.0).abs() < 1e-3);
    }

    #[test]
    fn opus_roundtrip() {
        let x: Vec<f32> = sine(48_000, 48_000.0);
        let file = klang_opus::encode_ogg_opus(&x, &x, 128, "Test").unwrap();
        let d = decode(&file).unwrap();
        assert_eq!(d.sample_rate, 48_000);
        assert!((d.samples.len() as i64 - 48_000).abs() < 1000, "{}", d.samples.len());
        let rms = (d.samples.iter().map(|v| v * v).sum::<f32>() / d.samples.len() as f32).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.05, "Pegel {rms}");
    }

    #[test]
    fn unsupported_formats_are_named() {
        let mut mp3 = vec![0xFF, 0xFB, 0x90, 0x64];
        mp3.extend(vec![0; 20]);
        assert!(decode(&mp3).unwrap_err().starts_with("Klangwelt can't read MP3 yet"));
        let mut id3 = b"ID3\x04\0\0\0\0\0\x02".to_vec();
        id3.extend(vec![0; 30]);
        assert!(decode(&id3).unwrap_err().starts_with("Klangwelt can't read MP3"));
        let mut m4a = b"\0\0\0\x20ftypM4A ".to_vec();
        m4a.extend(vec![0; 20]);
        assert!(decode(&m4a).unwrap_err().contains("AAC/M4A"));
        assert!(decode(b"hallo welt, keine musik").unwrap_err().starts_with("Unknown file format"));
        assert!(decode(b"").is_err());
    }
}
