//! The ZIP with the scores of a library package.
//!
//! Every score (MXL, MusicXML …) is stored as it is – MXL is compressed
//! already. The index notes for every score where its entry lies (offset of
//! the local header, length of header plus data, [`crate::index::DocInfo::zip`]),
//! so one range request fetches one score. The central directory at the end
//! makes every file reachable for ordinary ZIP tools, too. More than 65 535
//! entries or more than 4 GiB use the ZIP64 records.
//!
//! The writer leaves out everything that changes from build to build (times
//! are 1980-01-01), so the same scores give the same file.

use crate::source::{read_vec, ReadAt};
use std::io::{self, Read, Write};

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const END64: u32 = 0x0606_4b50;
const LOCATOR64: u32 = 0x0706_4b50;
/// File names are UTF-8.
const FLAG_UTF8: u16 = 0x0800;
/// 1980-01-01 in MS-DOS format.
const DOS_DATE: u16 = (1 << 5) | 1;
const LOCAL_LEN: u64 = 30;

struct Central {
    name: String,
    crc: u32,
    size: u64,
    offset: u64,
}

/// Writes a ZIP of stored entries.
pub struct ZipWriter<W: Write> {
    w: W,
    pos: u64,
    entries: Vec<Central>,
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(data);
    c.sum()
}

impl<W: Write> ZipWriter<W> {
    pub fn new(w: W) -> Self {
        Self { w, pos: 0, entries: Vec::new() }
    }

    /// Bytes written so far.
    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds a file; returns where it lies: [offset of the local header, length of header and data].
    pub fn add(&mut self, name: &str, data: &[u8]) -> io::Result<[u64; 2]> {
        let size = data.len() as u64;
        let big = size >= u32::MAX as u64;
        let crc = crc32(data);
        let mut h = Vec::with_capacity(64 + name.len());
        h.extend_from_slice(&LOCAL.to_le_bytes());
        h.extend_from_slice(&(if big { 45u16 } else { 20u16 }).to_le_bytes());
        h.extend_from_slice(&FLAG_UTF8.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&0u16.to_le_bytes()); // time
        h.extend_from_slice(&DOS_DATE.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        let short = if big { u32::MAX } else { size as u32 };
        h.extend_from_slice(&short.to_le_bytes());
        h.extend_from_slice(&short.to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&(if big { 20u16 } else { 0 }).to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        if big {
            h.extend_from_slice(&1u16.to_le_bytes());
            h.extend_from_slice(&16u16.to_le_bytes());
            h.extend_from_slice(&size.to_le_bytes());
            h.extend_from_slice(&size.to_le_bytes());
        }
        let offset = self.pos;
        self.w.write_all(&h)?;
        self.w.write_all(data)?;
        let total = h.len() as u64 + size;
        self.pos += total;
        self.entries.push(Central { name: name.to_string(), crc, size, offset });
        Ok([offset, total])
    }

    /// Writes the central directory and returns the inner writer.
    pub fn finish(mut self) -> io::Result<W> {
        let cd_start = self.pos;
        for e in &self.entries {
            let big_size = e.size >= u32::MAX as u64;
            let big_off = e.offset >= u32::MAX as u64;
            let mut extra = Vec::new();
            if big_size {
                extra.extend_from_slice(&e.size.to_le_bytes());
                extra.extend_from_slice(&e.size.to_le_bytes());
            }
            if big_off {
                extra.extend_from_slice(&e.offset.to_le_bytes());
            }
            let mut h = Vec::with_capacity(46 + e.name.len() + 32);
            h.extend_from_slice(&CENTRAL.to_le_bytes());
            let version: u16 = if extra.is_empty() { 20 } else { 45 };
            h.extend_from_slice(&version.to_le_bytes()); // made by (MS-DOS)
            h.extend_from_slice(&version.to_le_bytes()); // needed
            h.extend_from_slice(&FLAG_UTF8.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&DOS_DATE.to_le_bytes());
            h.extend_from_slice(&e.crc.to_le_bytes());
            let short = if big_size { u32::MAX } else { e.size as u32 };
            h.extend_from_slice(&short.to_le_bytes());
            h.extend_from_slice(&short.to_le_bytes());
            h.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            let extra_len = if extra.is_empty() { 0 } else { extra.len() + 4 };
            h.extend_from_slice(&(extra_len as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // comment
            h.extend_from_slice(&0u16.to_le_bytes()); // disk
            h.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            h.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            h.extend_from_slice(&(if big_off { u32::MAX } else { e.offset as u32 }).to_le_bytes());
            h.extend_from_slice(e.name.as_bytes());
            if !extra.is_empty() {
                h.extend_from_slice(&1u16.to_le_bytes());
                h.extend_from_slice(&(extra.len() as u16).to_le_bytes());
                h.extend_from_slice(&extra);
            }
            self.w.write_all(&h)?;
            self.pos += h.len() as u64;
        }
        let cd_size = self.pos - cd_start;
        let n = self.entries.len() as u64;
        let zip64 = n >= 0xFFFF || cd_start >= u32::MAX as u64 || cd_size >= u32::MAX as u64;
        if zip64 {
            let end64_at = self.pos;
            let mut r = Vec::with_capacity(76);
            r.extend_from_slice(&END64.to_le_bytes());
            r.extend_from_slice(&44u64.to_le_bytes());
            r.extend_from_slice(&45u16.to_le_bytes());
            r.extend_from_slice(&45u16.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes());
            r.extend_from_slice(&n.to_le_bytes());
            r.extend_from_slice(&n.to_le_bytes());
            r.extend_from_slice(&cd_size.to_le_bytes());
            r.extend_from_slice(&cd_start.to_le_bytes());
            r.extend_from_slice(&LOCATOR64.to_le_bytes());
            r.extend_from_slice(&0u32.to_le_bytes());
            r.extend_from_slice(&end64_at.to_le_bytes());
            r.extend_from_slice(&1u32.to_le_bytes());
            self.w.write_all(&r)?;
            self.pos += r.len() as u64;
        }
        let mut e = Vec::with_capacity(22);
        e.extend_from_slice(&END.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        let short_n = if zip64 { 0xFFFF } else { n as u16 };
        e.extend_from_slice(&short_n.to_le_bytes());
        e.extend_from_slice(&short_n.to_le_bytes());
        e.extend_from_slice(&(if zip64 { u32::MAX } else { cd_size as u32 }).to_le_bytes());
        e.extend_from_slice(&(if zip64 { u32::MAX } else { cd_start as u32 }).to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        self.w.write_all(&e)?;
        self.pos += e.len() as u64;
        self.w.flush()?;
        Ok(self.w)
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

fn corrupt() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, klang_i18n::tr!("MXL archive error: {e}", e = "scores.zip"))
}

/// Parses one entry (local header and data, as noted in the index): name and
/// contents. Checks the CRC; stored and deflated entries are understood.
pub fn parse_entry(raw: &[u8]) -> io::Result<(String, Vec<u8>)> {
    if raw.len() < LOCAL_LEN as usize || u32_at(raw, 0) != LOCAL {
        return Err(corrupt());
    }
    let method = u16_at(raw, 8);
    let crc = u32_at(raw, 14);
    let (name_len, extra_len) = (u16_at(raw, 26) as usize, u16_at(raw, 28) as usize);
    let start = LOCAL_LEN as usize + name_len + extra_len;
    if raw.len() < start {
        return Err(corrupt());
    }
    let name = String::from_utf8_lossy(&raw[30..30 + name_len]).into_owned();
    let mut csize = u32_at(raw, 18) as u64;
    if csize == u32::MAX as u64 {
        // ZIP64 extra field: uncompressed, compressed size
        let extra = &raw[30 + name_len..start];
        let mut i = 0;
        while i + 4 <= extra.len() {
            let (id, len) = (u16_at(extra, i), u16_at(extra, i + 2) as usize);
            if id == 1 && len >= 16 && i + 4 + len <= extra.len() {
                csize = u64_at(extra, i + 12);
            }
            i += 4 + len;
        }
    }
    let end = start.checked_add(usize::try_from(csize).map_err(io::Error::other)?).filter(|&e| e <= raw.len()).ok_or_else(corrupt)?;
    let data = &raw[start..end];
    let out = match method {
        0 => data.to_vec(),
        8 => {
            let mut v = Vec::new();
            flate2::read::DeflateDecoder::new(data).read_to_end(&mut v)?;
            v
        }
        _ => return Err(io::Error::new(io::ErrorKind::Unsupported, klang_i18n::tr!("Not supported: {e}", e = format!("ZIP method {method}")))),
    };
    if crc32(&out) != crc {
        return Err(corrupt());
    }
    Ok((name, out))
}

/// Reads one entry from the package's ZIP (see [`parse_entry`]).
pub fn read_entry(src: &dyn ReadAt, at: [u64; 2]) -> io::Result<(String, Vec<u8>)> {
    parse_entry(&read_vec(src, at[0], at[1])?)
}

/// An entry of the central directory: name, offset of the local header, stored size.
#[derive(Clone, Debug, PartialEq)]
pub struct DirEntry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

/// The central directory (at the end of the file) – to check a package.
pub fn read_directory(src: &dyn ReadAt) -> io::Result<Vec<DirEntry>> {
    let len = src.size()?;
    let tail_len = len.min(22 + 0xFFFF);
    let tail = read_vec(src, len - tail_len, tail_len)?;
    let at = (0..=tail.len().saturating_sub(22)).rev().find(|&i| u32_at(&tail, i) == END).ok_or_else(corrupt)?;
    let mut n = u16_at(&tail, at + 10) as u64;
    let mut cd_size = u32_at(&tail, at + 12) as u64;
    let mut cd_off = u32_at(&tail, at + 16) as u64;
    if (n == 0xFFFF || cd_off == u32::MAX as u64) && at >= 20 && u32_at(&tail, at - 20) == LOCATOR64 {
        let end64 = read_vec(src, u64_at(&tail, at - 20 + 8), 56)?;
        if u32_at(&end64, 0) != END64 {
            return Err(corrupt());
        }
        n = u64_at(&end64, 32);
        cd_size = u64_at(&end64, 40);
        cd_off = u64_at(&end64, 48);
    }
    let cd = read_vec(src, cd_off, cd_size)?;
    let mut out = Vec::with_capacity(n as usize);
    let mut i = 0;
    for _ in 0..n {
        if i + 46 > cd.len() || u32_at(&cd, i) != CENTRAL {
            return Err(corrupt());
        }
        let (name_len, extra_len, comment_len) = (u16_at(&cd, i + 28) as usize, u16_at(&cd, i + 30) as usize, u16_at(&cd, i + 32) as usize);
        let name = String::from_utf8_lossy(&cd[i + 46..i + 46 + name_len]).into_owned();
        let mut size = u32_at(&cd, i + 20) as u64;
        let mut offset = u32_at(&cd, i + 42) as u64;
        let extra = &cd[i + 46 + name_len..i + 46 + name_len + extra_len];
        let mut j = 0;
        while j + 4 <= extra.len() {
            let (id, len) = (u16_at(extra, j), u16_at(extra, j + 2) as usize);
            if id == 1 {
                let mut k = j + 4;
                if u32_at(&cd, i + 24) == u32::MAX {
                    k += 8;
                }
                if size == u32::MAX as u64 {
                    size = u64_at(extra, k);
                    k += 8;
                }
                if offset == u32::MAX as u64 {
                    offset = u64_at(extra, k);
                }
            }
            j += 4 + len;
        }
        out.push(DirEntry { name, offset, size });
        i += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_found_by_offset_and_by_directory_and_other_readers_agree() {
        let mut w = ZipWriter::new(Vec::new());
        let a = w.add("pdmx/Qm1.mxl", b"erste Partitur").unwrap();
        let b = w.add("openscore/Lieder/Schubert, Franz – Ständchen.mxl", &vec![7u8; 5000]).unwrap();
        let bytes = w.finish().unwrap();
        assert_eq!(read_entry(&bytes, a).unwrap(), ("pdmx/Qm1.mxl".to_string(), b"erste Partitur".to_vec()));
        let (name, data) = read_entry(&bytes, b).unwrap();
        assert!(name.contains("Ständchen") && data.len() == 5000);
        let dir = read_directory(&bytes).unwrap();
        assert_eq!(dir.iter().map(|e| (e.offset, e.size)).collect::<Vec<_>>(), vec![(a[0], 14), (b[0], 5000)]);
        // the zip crate (another reader) sees the same files
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        assert_eq!(z.len(), 2);
        let mut s = String::new();
        z.by_name("pdmx/Qm1.mxl").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "erste Partitur");
        // a damaged byte is noticed
        let mut bad = bytes.clone();
        bad[(a[0] + a[1] - 1) as usize] ^= 1;
        assert!(read_entry(&bad, a).is_err());
    }

    #[test]
    fn many_entries_use_zip64() {
        let mut w = ZipWriter::new(Vec::new());
        let mut last = [0, 0];
        for i in 0..70_000u32 {
            last = w.add(&format!("{i}.xml"), &i.to_le_bytes()).unwrap();
        }
        let bytes = w.finish().unwrap();
        let dir = read_directory(&bytes).unwrap();
        assert_eq!(dir.len(), 70_000);
        assert_eq!(dir[69_999].offset, last[0]);
        assert_eq!(read_entry(&bytes, last).unwrap().1, 69_999u32.to_le_bytes().to_vec());
        let z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(z.len(), 70_000);
    }
}
