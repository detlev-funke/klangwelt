//! Saving and opening projects.
//!
//! A `.klangwelt` file is a ZIP container:
//! * `projekt.json` – project (tracks, clips, notes with expression, sounds)
//!   and a list of the audio recordings,
//! * `audio/<id>.wav` – each recording used, as WAV (32-bit float, mono,
//!   lossless).
//!
//! It is written to a temporary file first, which then replaces the old one: a
//! crash while saving never destroys the last good project.

use klang_core::{ClipSource, Project};
use klang_i18n::tr;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const EXT: &str = "klangwelt";
const FORMAT: &str = "klangwelt-projekt";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format: String,
    version: u32,
    project: Project,
    assets: Vec<AssetEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetEntry {
    id: String,
    name: String,
    sample_rate: u32,
    frames: usize,
    file: String,
}

/// An audio recording for saving or from a file.
pub struct AudioData {
    pub id: String,
    pub name: String,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// IDs of the recordings the project refers to.
pub fn used_assets(p: &Project) -> Vec<String> {
    let mut ids: Vec<String> = p
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter_map(|c| match &c.source {
            ClipSource::Audio { asset_id, .. } => Some(asset_id.clone()),
            _ => None,
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

// ---------------------------------------------------------------- WAV

/// WAV, 32-bit float (format tag 3), mono.
pub fn write_wav(w: &mut impl Write, samples: &[f32], sample_rate: u32) -> io::Result<()> {
    let data = (samples.len() * 4) as u32;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&3u16.to_le_bytes())?; // IEEE float
    w.write_all(&1u16.to_le_bytes())?; // mono
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&(sample_rate * 4).to_le_bytes())?;
    w.write_all(&4u16.to_le_bytes())?;
    w.write_all(&32u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data.to_le_bytes())?;
    let mut buf = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    w.write_all(&buf)
}

/// Reads WAV: 32-bit float or 16-bit PCM, multi-channel is averaged to mono.
pub fn read_wav(bytes: &[u8]) -> io::Result<(Vec<f32>, u32)> {
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, format!("WAV: {m}"));
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad(tr!("not a WAV file")));
    }
    let (mut i, mut fmt, mut data) = (12usize, None, None);
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        let body = bytes.get(i + 8..i + 8 + len).ok_or_else(|| bad(tr!("chunk truncated")))?;
        match id {
            b"fmt " if len >= 16 => {
                let u16_at = |o: usize| u16::from_le_bytes(body[o..o + 2].try_into().unwrap());
                fmt = Some((u16_at(0), u16_at(2).max(1), u32::from_le_bytes(body[4..8].try_into().unwrap()), u16_at(14)));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        i += 8 + len + (len & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or_else(|| bad(tr!("fmt missing")))?;
    let data = data.ok_or_else(|| bad(tr!("data missing")))?;
    let frames: Vec<f32> = match (format, bits) {
        (3, 32) => data.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
        (1, 16) => data.chunks_exact(2).map(|c| i16::from_le_bytes(c.try_into().unwrap()) as f32 / 32768.0).collect(),
        _ => return Err(bad(tr!("only 32-bit float or 16-bit PCM"))),
    };
    let ch = channels as usize;
    let mono = if ch == 1 { frames } else { frames.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect() };
    Ok((mono, rate))
}

// ---------------------------------------------------------------- File

/// Writes the ZIP container to `w` and returns the writer afterwards.
fn write_zip<W: Write + io::Seek>(w: W, project: &Project, audio: &[AudioData]) -> io::Result<W> {
    let mut zip = zip::ZipWriter::new(w);
    let json_opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let raw_opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    let mut entries = Vec::new();
    for a in audio {
        let file = format!("audio/{}.wav", sanitize(&a.id));
        zip.start_file(file.as_str(), raw_opts).map_err(io::Error::other)?;
        write_wav(&mut zip, &a.samples, a.sample_rate)?;
        entries.push(AssetEntry { id: a.id.clone(), name: a.name.clone(), sample_rate: a.sample_rate, frames: a.samples.len(), file });
    }
    let manifest = Manifest { format: FORMAT.into(), version: VERSION, project: project.clone(), assets: entries };
    zip.start_file("projekt.json", json_opts).map_err(io::Error::other)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?)?;
    zip.finish().map_err(io::Error::other)
}

/// Writes the project with its recordings to `path` (atomically via a temporary file).
#[cfg(not(target_arch = "wasm32"))]
pub fn save(path: &Path, project: &Project, audio: &[AudioData]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension(format!("{EXT}.teil"));
    let result = (|| -> io::Result<()> {
        let file = std::fs::File::create(&part)?;
        let mut inner = write_zip(io::BufWriter::new(file), project, audio)?;
        inner.flush()?;
        inner.into_inner().map_err(|e| e.into_error())?.sync_all()
    })();
    match result {
        Ok(()) => std::fs::rename(&part, path),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// In the browser: the container is built in memory and ends up in the core's
/// file tree (the web core backs it up in OPFS).
#[cfg(target_arch = "wasm32")]
pub fn save(path: &Path, project: &Project, audio: &[AudioData]) -> io::Result<()> {
    let bytes = write_zip(io::Cursor::new(Vec::new()), project, audio)?.into_inner();
    crate::vfs::write_atomic(path, &bytes, &format!("{EXT}.teil"))
}

/// Reads a project with its recordings.
pub fn load(path: &Path) -> io::Result<(Project, Vec<AudioData>)> {
    #[cfg(not(target_arch = "wasm32"))]
    let source = io::BufReader::new(std::fs::File::open(path)?);
    #[cfg(target_arch = "wasm32")]
    let source = io::Cursor::new(crate::vfs::read(path)?);
    read_zip(source)
}

/// Reads the ZIP container (project file as bytes, e.g. on import).
pub fn read_zip<R: Read + io::Seek>(source: R) -> io::Result<(Project, Vec<AudioData>)> {
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidData, m);
    let mut zip = zip::ZipArchive::new(source).map_err(|e| bad(tr!("not a Klangwelt project file ({error})", error = e)))?;
    let manifest: Manifest = {
        let mut f = zip.by_name("projekt.json").map_err(|_| bad(tr!("projekt.json missing")))?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        serde_json::from_str(&s).map_err(|e| bad(format!("projekt.json: {e}")))?
    };
    if manifest.format != FORMAT {
        return Err(bad(tr!("not a Klangwelt project file")));
    }
    if manifest.version > VERSION {
        return Err(bad(tr!("Project was created with a newer version of Klangwelt (format {version})", version = manifest.version)));
    }
    let mut audio = Vec::new();
    for a in &manifest.assets {
        let mut f = zip.by_name(&a.file).map_err(|_| bad(tr!("Recording {file} missing", file = a.file)))?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        let (samples, rate) = read_wav(&bytes)?;
        audio.push(AudioData { id: a.id.clone(), name: a.name.clone(), samples, sample_rate: rate });
    }
    Ok((manifest.project, audio))
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// File name from a project name (without forbidden characters).
pub fn file_name_for(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim_matches(|c: char| c == '.' || c.is_whitespace());
    format!("{}.{EXT}", if clean.is_empty() { "Projekt" } else { clean })
}

/// Input → path: a bare name ends up in `dir`, otherwise the path is used
/// (extension is added).
pub fn resolve(input: &str, dir: &Path) -> PathBuf {
    let s = input.trim().trim_matches('"');
    let looks_like_path = s.contains('/') || s.contains('\\') || s.contains(':');
    let p = if looks_like_path { PathBuf::from(s) } else { dir.join(file_name_for(s)) };
    if p.extension().map(|e| e.eq_ignore_ascii_case(EXT)).unwrap_or(false) {
        p
    } else {
        let mut q = p.into_os_string();
        q.push(format!(".{EXT}"));
        PathBuf::from(q)
    }
}

/// A project file in the project folder.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub name: String,
    pub path: String,
    /// Unix time in milliseconds
    pub modified: u64,
    pub bytes: u64,
}

/// Projects in the folder `dir`, most recently modified first.
pub fn list(dir: &Path) -> Vec<ProjectFile> {
    let mut out: Vec<ProjectFile> = crate::vfs::list(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.extension().map(|x| x.eq_ignore_ascii_case(EXT)).unwrap_or(false))
        .map(|e| ProjectFile {
            name: e.path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(),
            path: e.path.display().to_string(),
            modified: e.modified_ms,
            bytes: e.len,
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use klang_core::{Clip, Id, Note, Track, TrackKind};

    #[test]
    fn wav_roundtrip_and_pcm16() {
        let s: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.01).sin() * 0.8).collect();
        let mut buf = Vec::new();
        write_wav(&mut buf, &s, 48_000).unwrap();
        let (back, rate) = read_wav(&buf).unwrap();
        assert_eq!((back, rate), (s, 48_000));
        // 16-bit PCM stereo becomes mono
        let mut pcm = Vec::new();
        pcm.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        pcm.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, 2] {
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        pcm.extend_from_slice(&44_100u32.to_le_bytes());
        pcm.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        pcm.extend_from_slice(&4u16.to_le_bytes());
        pcm.extend_from_slice(&16u16.to_le_bytes());
        pcm.extend_from_slice(b"data");
        pcm.extend_from_slice(&8u32.to_le_bytes());
        for v in [16384i16, 0, -32768, 0] {
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        let (mono, rate) = read_wav(&pcm).unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(mono, vec![0.25, -0.5]);
    }

    #[test]
    fn project_roundtrip_with_audio_and_expression() {
        let mut p = Project::default();
        p.name = "Test".into();
        let mut audio_track = Track::new(Id(1), "Gesang", TrackKind::Audio);
        audio_track.clips.push(Clip {
            id: Id(2),
            name: "Aufnahme".into(),
            start_beats: 4.0,
            source: ClipSource::Audio { asset_id: "asset-3".into(), source_bpm: 100.0, warp: true, duration_secs: 0.5 },
        });
        let mut melody = Track::new(Id(4), "Melodie", TrackKind::Instrument);
        melody.clips.push(Clip {
            id: Id(5),
            name: "Thema".into(),
            start_beats: 0.0,
            source: ClipSource::Notes { length_beats: 4.0, notes: vec![Note { vibrato: 0.3, bend: -2.0, ..Note::new(60, 0.0, 1.0, 90) }] },
        });
        p.tracks = vec![audio_track, melody];
        let samples: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let audio = vec![AudioData { id: "asset-3".into(), name: "Aufnahme 1".into(), samples: samples.clone(), sample_rate: 48_000 }];
        assert_eq!(used_assets(&p), vec!["asset-3".to_string()]);

        let dir = std::env::temp_dir().join(format!("klang-projekt-{}", std::process::id()));
        let path = resolve("Mein Lied", &dir);
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "Mein Lied.klangwelt");
        save(&path, &p, &audio).unwrap();
        assert!(!path.with_extension("klangwelt.teil").exists());
        let (back, a) = load(&path).unwrap();
        assert_eq!(back, p);
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].id.as_str(), a[0].name.as_str(), a[0].sample_rate), ("asset-3", "Aufnahme 1", 48_000));
        assert_eq!(a[0].samples, samples);
        // Overwriting replaces the old file
        p.name = "Test 2".into();
        save(&path, &p, &audio).unwrap();
        assert_eq!(load(&path).unwrap().0.name, "Test 2");
        // Foreign files are rejected
        std::fs::write(dir.join("kaputt.klangwelt"), b"kein zip").unwrap();
        assert!(load(&dir.join("kaputt.klangwelt")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn names_and_paths() {
        let dir = Path::new("C:/Projekte");
        assert_eq!(file_name_for("A/B:C?"), "A_B_C_.klangwelt");
        assert_eq!(file_name_for("  "), "Projekt.klangwelt");
        assert_eq!(resolve("Lied", dir), dir.join("Lied.klangwelt"));
        assert_eq!(resolve("D:\\x\\y.klangwelt", dir), PathBuf::from("D:\\x\\y.klangwelt"));
        assert_eq!(resolve("D:\\x\\y", dir), PathBuf::from("D:\\x\\y.klangwelt"));
    }
}
