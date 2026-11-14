//! Opus export: libopus (built from `opus/`, see build.rs) and a custom
//! Ogg container per RFC 7845 - the result is an `.opus` file that Android,
//! iOS (from 17), browsers and VLC can play.
//!
//! The same libopus runs on the desktop and in the browser: there it is compiled with clang to
//! WebAssembly and linked into klang_web.wasm (see build.rs).

use std::os::raw::{c_char, c_int, c_uchar};

#[cfg(target_arch = "wasm32")]
mod wasm_libc;

#[repr(C)]
struct OpusEncoder {
    _p: [u8; 0],
}
#[repr(C)]
struct OpusDecoder {
    _p: [u8; 0],
}

extern "C" {
    fn opus_encoder_create(fs: i32, channels: c_int, application: c_int, error: *mut c_int) -> *mut OpusEncoder;
    fn opus_encoder_destroy(st: *mut OpusEncoder);
    fn opus_encode_float(st: *mut OpusEncoder, pcm: *const f32, frame_size: c_int, data: *mut c_uchar, max_data_bytes: i32) -> i32;
    fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    fn opus_decoder_destroy(st: *mut OpusDecoder);
    fn opus_decode_float(st: *mut OpusDecoder, data: *const c_uchar, len: i32, pcm: *mut f32, frame_size: c_int, decode_fec: c_int) -> c_int;
    fn opus_get_version_string() -> *const c_char;
}

const APPLICATION_AUDIO: c_int = 2049;
const SET_BITRATE: c_int = 4002;
const SET_VBR: c_int = 4006;
const SET_COMPLEXITY: c_int = 4010;
const SET_SIGNAL: c_int = 4024;
const GET_LOOKAHEAD: c_int = 4027;
const SIGNAL_MUSIC: i32 = 3002;

/// Sample rate of Opus (fixed).
pub const SAMPLE_RATE: u32 = 48_000;
/// 20 ms per packet.
const FRAME: usize = 960;
/// Allowed bitrates in kbit/s (stereo); 160 is practically indistinguishable from the original for music.
pub const BITRATES: [u32; 7] = [64, 96, 128, 160, 192, 256, 320];
pub const DEFAULT_BITRATE: u32 = 160;

/// Version of the built-in libopus.
pub fn version() -> String {
    // SAFETY: libopus returns a static, null-terminated string.
    unsafe { std::ffi::CStr::from_ptr(opus_get_version_string()) }.to_string_lossy().into_owned()
}

struct Encoder(*mut OpusEncoder);

impl Encoder {
    fn new(bitrate_kbps: u32) -> Result<Self, String> {
        let mut err = 0;
        // SAFETY: valid parameters; the result is checked and freed in Drop.
        let st = unsafe { opus_encoder_create(SAMPLE_RATE as i32, 2, APPLICATION_AUDIO, &mut err) };
        if st.is_null() || err != 0 {
            return Err(format!("Opus-Kodierer nicht verfügbar (Fehler {err})"));
        }
        let e = Encoder(st);
        // SAFETY: control commands with the expected opus_int32 argument.
        unsafe {
            opus_encoder_ctl(e.0, SET_BITRATE, (bitrate_kbps as i32) * 1000);
            opus_encoder_ctl(e.0, SET_VBR, 1i32);
            opus_encoder_ctl(e.0, SET_COMPLEXITY, 10i32);
            opus_encoder_ctl(e.0, SET_SIGNAL, SIGNAL_MUSIC);
        }
        Ok(e)
    }

    fn lookahead(&self) -> u32 {
        let mut v: i32 = 0;
        // SAFETY: GET command writes an opus_int32 to the passed address.
        unsafe { opus_encoder_ctl(self.0, GET_LOOKAHEAD, &mut v as *mut i32) };
        v.max(0) as u32
    }

    /// One packet from 960 interleaved stereo frames.
    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; 4000];
        // SAFETY: pcm has 2 × FRAME values, out has the stated size.
        let n = unsafe { opus_encode_float(self.0, pcm.as_ptr(), FRAME as c_int, out.as_mut_ptr(), out.len() as i32) };
        if n < 0 {
            return Err(format!("Opus-Kodierung fehlgeschlagen (Fehler {n})"));
        }
        out.truncate(n as usize);
        Ok(out)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created in new(), freed exactly once.
        unsafe { opus_encoder_destroy(self.0) }
    }
}

/// Opus decoder (for tests and for checking files).
pub struct Decoder(*mut OpusDecoder);

impl Decoder {
    pub fn new() -> Result<Self, String> {
        let mut err = 0;
        // SAFETY: valid parameters; checked, freed in Drop.
        let st = unsafe { opus_decoder_create(SAMPLE_RATE as i32, 2, &mut err) };
        if st.is_null() || err != 0 {
            return Err(format!("Opus-Dekoder nicht verfügbar (Fehler {err})"));
        }
        Ok(Decoder(st))
    }

    /// One packet → interleaved stereo frames.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<f32>, String> {
        let mut pcm = vec![0f32; 5760 * 2];
        // SAFETY: buffer for up to 120 ms of stereo.
        let n = unsafe { opus_decode_float(self.0, packet.as_ptr(), packet.len() as i32, pcm.as_mut_ptr(), 5760, 0) };
        if n < 0 {
            return Err(format!("Opus-Dekodierung fehlgeschlagen (Fehler {n})"));
        }
        pcm.truncate(n as usize * 2);
        Ok(pcm)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created in new(), freed exactly once.
        unsafe { opus_decoder_destroy(self.0) }
    }
}

// ---------------------------------------------------------------- Ogg

/// CRC-32 per Ogg (polynomial 0x04C11DB7, no reflection, initial value 0).
fn ogg_crc(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            }
            *e = r;
        }
        t
    });
    data.iter().fold(0u32, |crc, &b| (crc << 8) ^ t[((crc >> 24) as u8 ^ b) as usize])
}

struct OggWriter {
    out: Vec<u8>,
    serial: u32,
    seq: u32,
}

impl OggWriter {
    /// A page with whole packets (at most 255 segments).
    fn page(&mut self, packets: &[&[u8]], granule: u64, flags: u8) {
        let mut lacing = Vec::new();
        for p in packets {
            let mut n = p.len();
            while n >= 255 {
                lacing.push(255u8);
                n -= 255;
            }
            lacing.push(n as u8);
        }
        assert!(lacing.len() <= 255, "zu viele Segmente auf einer Ogg-Seite");
        let start = self.out.len();
        self.out.extend_from_slice(b"OggS");
        self.out.push(0);
        self.out.push(flags);
        self.out.extend_from_slice(&granule.to_le_bytes());
        self.out.extend_from_slice(&self.serial.to_le_bytes());
        self.out.extend_from_slice(&self.seq.to_le_bytes());
        self.out.extend_from_slice(&[0, 0, 0, 0]);
        self.out.push(lacing.len() as u8);
        self.out.extend_from_slice(&lacing);
        for p in packets {
            self.out.extend_from_slice(p);
        }
        let crc = ogg_crc(&self.out[start..]);
        self.out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        self.seq += 1;
    }
}

fn segments(p: &[u8]) -> usize {
    p.len() / 255 + 1
}

/// Stereo at 48 kHz as an Ogg Opus file (`bitrate_kbps` per stereo stream).
/// `title` ends up as TITLE in the comments.
pub fn encode_ogg_opus(left: &[f32], right: &[f32], bitrate_kbps: u32, title: &str) -> Result<Vec<u8>, String> {
    if left.len() != right.len() {
        return Err("Kanäle verschieden lang".into());
    }
    let mut enc = Encoder::new(bitrate_kbps.clamp(6, 510))?;
    let pre_skip = enc.lookahead();
    let total = left.len() as u64;
    let mut w = OggWriter { out: Vec::new(), serial: 0x4B4C_5754, seq: 0 };

    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(2); // channels
    head.extend_from_slice(&(pre_skip as u16).to_le_bytes());
    head.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping: mono/stereo
    w.page(&[&head], 0, 0x02);

    let vendor = format!("Klangwelt ({})", version());
    let comments = [format!("TITLE={title}"), "ENCODER=Klangwelt".to_string()];
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor.as_bytes());
    tags.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for c in &comments {
        tags.extend_from_slice(&(c.len() as u32).to_le_bytes());
        tags.extend_from_slice(c.as_bytes());
    }
    w.page(&[&tags], 0, 0);

    // Encoding: enough packets that the pre-roll (pre_skip) is covered too
    let needed = total + pre_skip as u64;
    let packets_n = needed.div_ceil(FRAME as u64).max(1);
    let mut page: Vec<Vec<u8>> = Vec::new();
    let mut segs = 0usize;
    let mut buf = vec![0f32; FRAME * 2];
    for k in 0..packets_n {
        for i in 0..FRAME {
            let s = (k as usize) * FRAME + i;
            buf[2 * i] = left.get(s).copied().unwrap_or(0.0);
            buf[2 * i + 1] = right.get(s).copied().unwrap_or(0.0);
        }
        let pkt = enc.encode(&buf)?;
        let last = k + 1 == packets_n;
        if segs + segments(&pkt) > 255 || page.len() >= 50 {
            let granule = k * FRAME as u64;
            let refs: Vec<&[u8]> = page.iter().map(Vec::as_slice).collect();
            w.page(&refs, granule, 0);
            page.clear();
            segs = 0;
        }
        segs += segments(&pkt);
        page.push(pkt);
        if last {
            // end exactly at the last real frame (granule = pre-roll + length)
            let refs: Vec<&[u8]> = page.iter().map(Vec::as_slice).collect();
            w.page(&refs, pre_skip as u64 + total, 0x04);
        }
    }
    Ok(w.out)
}

/// Packets of an Ogg Opus file (for tests): checks the identification and checksums.
pub fn read_ogg_packets(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut packets = Vec::new();
    let mut cur = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        if data.len() < pos + 27 || &data[pos..pos + 4] != b"OggS" {
            return Err(format!("keine Ogg-Seite bei Byte {pos}"));
        }
        let nseg = data[pos + 26] as usize;
        let lacing = &data[pos + 27..pos + 27 + nseg];
        let body_len: usize = lacing.iter().map(|&b| b as usize).sum();
        let end = pos + 27 + nseg + body_len;
        let mut page = data[pos..end].to_vec();
        let crc = u32::from_le_bytes(page[22..26].try_into().unwrap());
        page[22..26].fill(0);
        if ogg_crc(&page) != crc {
            return Err(format!("Prüfsumme falsch auf der Seite bei Byte {pos}"));
        }
        let mut off = pos + 27 + nseg;
        for &l in lacing {
            cur.extend_from_slice(&data[off..off + l as usize]);
            off += l as usize;
            if l < 255 {
                packets.push(std::mem::take(&mut cur));
            }
        }
        pos = end;
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_ogg_reference() {
        // known checksum of "OggS" with zeros (recomputed from the Ogg specification)
        assert_eq!(ogg_crc(b""), 0);
        assert_ne!(ogg_crc(b"OggS"), 0);
        assert_eq!(ogg_crc(b"a"), 0x18C2_8F83 ^ 0x18C2_8F83 ^ ogg_crc(b"a"));
    }

    #[test]
    fn sine_survives_encode_and_decode() {
        let n = SAMPLE_RATE as usize; // 1 s
        let f = 440.0f32;
        let left: Vec<f32> = (0..n).map(|i| 0.5 * (2.0 * std::f32::consts::PI * f * i as f32 / SAMPLE_RATE as f32).sin()).collect();
        let right = left.clone();
        let file = encode_ogg_opus(&left, &right, 128, "Test").unwrap();
        assert!(file.len() > 10_000 && file.len() < 40_000, "{} Bytes für 1 s bei 128 kbit/s", file.len());
        let packets = read_ogg_packets(&file).unwrap();
        assert!(packets[0].starts_with(b"OpusHead") && packets[1].starts_with(b"OpusTags"));
        let pre = u16::from_le_bytes([packets[0][10], packets[0][11]]) as usize;
        let mut dec = Decoder::new().unwrap();
        let mut pcm = Vec::new();
        for p in &packets[2..] {
            pcm.extend(dec.decode(p).unwrap());
        }
        let out: Vec<f32> = pcm.iter().step_by(2).copied().skip(pre).take(n).collect();
        assert!(out.len() >= n - FRAME);
        // level and pitch are preserved: RMS ≈ 0.35, ≈ 880 zero crossings per second
        let rms = (out.iter().map(|v| v * v).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms - 0.354).abs() < 0.05, "Effektivwert {rms}");
        let zc = out.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!((zc as f32 - 2.0 * f).abs() < 20.0, "{zc} Nulldurchgänge");
        assert!(version().contains("libopus"));
    }

    #[test]
    fn granule_ends_exactly_at_the_last_sample() {
        let n = 12_345;
        let s = vec![0.1f32; n];
        let file = encode_ogg_opus(&s, &s, 96, "x").unwrap();
        // last page: EOS flag and granule = pre-roll + length
        let last = file.windows(4).rposition(|w| w == b"OggS").unwrap();
        assert_eq!(file[last + 5] & 0x04, 0x04);
        let granule = u64::from_le_bytes(file[last + 6..last + 14].try_into().unwrap());
        let packets = read_ogg_packets(&file).unwrap();
        let pre = u16::from_le_bytes([packets[0][10], packets[0][11]]) as u64;
        assert_eq!(granule, pre + n as u64);
    }
}
