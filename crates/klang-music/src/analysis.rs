//! Analysis of a score: voice functions, cadences, mood.
//!
//! Stage 1 is comprehensible heuristics. They provide the first
//! (weak) labels, with which a model is later trained and
//! corrected by hand or by the agent.

use crate::melody::{features, skyline, Feature, MelodyNote};
use crate::score::{Score, Voice};
use crate::theory::{estimate_key_from_histogram, key_from_fifths_guess, Key};
use klang_i18n::{tid, tk};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VoiceRole {
    MainTheme,
    CounterMelody,
    InnerVoice,
    Accompaniment,
    Ostinato,
    Bass,
}

impl VoiceRole {
    /// Label (a key for `klang_i18n::translate`).
    pub fn label_de(self) -> &'static str {
        match self {
            VoiceRole::MainTheme => tk!("Main theme"),
            VoiceRole::CounterMelody => tk!("Countermelody"),
            VoiceRole::InnerVoice => tk!("Inner voice"),
            VoiceRole::Accompaniment => tk!("Accompaniment figure"),
            VoiceRole::Ostinato => tk!("Ostinato"),
            VoiceRole::Bass => tk!("Bass line"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceProfile {
    pub part: String,
    pub staff: u32,
    pub voice: u32,
    pub note_count: usize,
    pub mean_pitch: f32,
    pub range: u8,
    /// Onsets per quarter note while the voice is playing.
    pub density: f32,
    /// Share of the piece's duration in which the voice sounds.
    pub coverage: f32,
    /// Share of steps ≤ whole tone.
    pub stepwise: f32,
    /// Share of onsets with several notes.
    pub chordal: f32,
    /// 0…1 – how strongly a pattern repeats.
    pub repetition: f32,
    /// Length of the repeated pattern in notes.
    pub period: Option<usize>,
    pub rhythmic_variety: f32,
    pub salience: f32,
    pub role: VoiceRole,
    /// Label of the role for display (translated).
    pub role_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CadenceKind {
    /// V → I in the bass, upper voice ends on the tonic.
    Authentic,
    /// V → I in the bass, upper voice not on the tonic.
    ImperfectAuthentic,
    /// Longer dwelling on the dominant at the phrase end.
    Half,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cadence {
    /// Position in quarter notes.
    pub onset: f64,
    pub kind: CadenceKind,
    pub label: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mood {
    /// −1 (sad/dark) … +1 (happy/bright)
    pub valence: f32,
    /// −1 (calm) … +1 (energetic)
    pub arousal: f32,
    /// German keyword – stored like this in the index facets (an identifier for
    /// `klang_i18n::translate`).
    pub label: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreAnalysis {
    pub key: Option<Key>,
    /// German name of the key ("Es-Dur") – stored like this in the facets of the
    /// index; for display `key.map(|k| k.name())`.
    pub key_name: Option<String>,
    pub tempo_bpm: Option<f64>,
    pub duration_quarters: f64,
    pub chromaticism: f32,
    pub mood: Mood,
    pub voices: Vec<VoiceProfile>,
    pub cadences: Vec<Cadence>,
}

struct Raw<'a> {
    part: &'a str,
    voice: &'a Voice,
    line: Vec<MelodyNote>,
    feats: Vec<Feature>,
}

fn repetition(f: &[Feature]) -> (f32, Option<usize>) {
    let mut best = 0.0f32;
    let mut period = None;
    for p in 1..=16 {
        if f.len() < 4 * p {
            break;
        }
        let n = f.len() - p;
        let same = (0..n).filter(|&i| f[i] == f[i + p]).count();
        let ratio = same as f32 / n as f32;
        if ratio > best {
            best = ratio;
        }
        if ratio >= 0.85 && period.is_none() {
            period = Some(p);
        }
    }
    (best, period)
}

fn entropy_norm(values: impl Iterator<Item = i8>) -> f32 {
    let mut counts = [0usize; 9];
    let mut total = 0;
    for v in values {
        counts[(v.clamp(-4, 4) + 4) as usize] += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    let h: f32 = counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f32 / total as f32;
            -p * p.log2()
        })
        .sum();
    h / 9f32.log2()
}

fn sounding_at(line: &[MelodyNote], t: f64) -> Option<&MelodyNote> {
    line.iter().rev().find(|n| n.onset <= t + 1e-6 && n.onset + n.duration > t - 1e-6)
}

pub fn analyze_score(score: &Score) -> ScoreAnalysis {
    analyze_score_with(score, true)
}

/// Like [`analyze_score`]; without cadence search (`cadences = false`) significantly
/// faster – enough for the features of the tree view.
/// Key of a score: from the key signature (mode guessed if necessary),
/// otherwise from the pitch distribution.
pub fn score_key(score: &Score) -> Option<Key> {
    score_key_hist(score).0
}

fn score_key_hist(score: &Score) -> (Option<Key>, [f64; 12]) {
    let mut hist = [0.0f64; 12];
    for n in score.parts.iter().flat_map(|p| &p.voices).flat_map(|v| &v.notes) {
        hist[n.midi as usize % 12] += n.duration;
    }
    let key = match (score.key_fifths, score.key_mode.as_deref()) {
        (Some(f), Some("minor")) => Some(Key::from_fifths(f, true)),
        (Some(f), Some("major")) => Some(Key::from_fifths(f, false)),
        // Key signature without mode: guess major or the relative minor key.
        (Some(f), _) => {
            let notes = score.parts.iter().flat_map(|p| &p.voices).flat_map(|v| &v.notes);
            let last = notes.clone().map(|n| n.onset).fold(f64::MIN, f64::max);
            let final_bass = notes.filter(|n| (n.onset - last).abs() < 1e-6).map(|n| n.midi).min().map(|m| m % 12);
            Some(key_from_fifths_guess(f, &hist, final_bass))
        }
        (None, _) => estimate_key_from_histogram(&hist),
    };
    (key, hist)
}

pub fn analyze_score_with(score: &Score, with_cadences: bool) -> ScoreAnalysis {
    let total = score.duration_quarters().max(1e-6);

    // Key: from the score, otherwise estimated.
    let (key, hist) = score_key_hist(score);
    let hist_total: f64 = hist.iter().sum();
    let chromaticism = match key {
        Some(k) if hist_total > 0.0 => {
            (hist.iter().enumerate().filter(|(pc, _)| !k.contains(*pc as u8)).map(|(_, d)| d).sum::<f64>() / hist_total) as f32
        }
        _ => 0.0,
    };

    let raws: Vec<Raw> = score
        .parts
        .iter()
        .flat_map(|p| p.voices.iter().map(move |v| (p.name.as_str(), v)))
        .filter(|(_, v)| !v.notes.is_empty())
        .map(|(part, voice)| {
            let line = skyline(&voice.notes);
            let feats = features(&line);
            Raw { part, voice, line, feats }
        })
        .collect();

    let mean_pitches: Vec<f32> = raws
        .iter()
        .map(|r| r.voice.notes.iter().map(|n| n.midi as f32).sum::<f32>() / r.voice.notes.len() as f32)
        .collect();

    let mut profiles: Vec<VoiceProfile> = raws
        .iter()
        .zip(&mean_pitches)
        .map(|(r, &mean_pitch)| {
            let notes = &r.voice.notes;
            let lo = notes.iter().map(|n| n.midi).min().unwrap_or(0);
            let hi = notes.iter().map(|n| n.midi).max().unwrap_or(0);
            let first = r.line.first().map(|n| n.onset).unwrap_or(0.0);
            let last = r.line.last().map(|n| n.onset + n.duration).unwrap_or(0.0);
            let span = (last - first).max(1e-6);
            let sounding: f64 = r
                .line
                .iter()
                .enumerate()
                .map(|(i, n)| match r.line.get(i + 1) {
                    Some(next) => n.duration.min(next.onset - n.onset),
                    None => n.duration,
                })
                .sum();
            let stepwise = if r.feats.is_empty() {
                0.0
            } else {
                r.feats.iter().filter(|f| f.interval.abs() <= 2).count() as f32 / r.feats.len() as f32
            };
            let (rep, period) = repetition(&r.feats);
            VoiceProfile {
                part: r.part.to_string(),
                staff: r.voice.staff,
                voice: r.voice.voice,
                note_count: notes.len(),
                mean_pitch,
                range: hi - lo,
                density: (r.line.len() as f64 / span) as f32,
                coverage: (sounding / total).min(1.0) as f32,
                stepwise,
                chordal: 1.0 - r.line.len() as f32 / notes.len().max(1) as f32,
                repetition: rep,
                period,
                rhythmic_variety: entropy_norm(r.feats.iter().map(|f| f.rhythm)),
                salience: 0.0,
                role: VoiceRole::InnerVoice,
                role_label: String::new(),
            }
        })
        .collect();

    // Salience: high register, little repetition, rhythmically lively, stepwise, present.
    let n = profiles.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| profiles[a].mean_pitch.total_cmp(&profiles[b].mean_pitch));
    for (rank, &i) in order.iter().enumerate() {
        let p = &mut profiles[i];
        let pitch_rank = if n > 1 { rank as f32 / (n - 1) as f32 } else { 1.0 };
        p.salience = 0.35 * pitch_rank + 0.25 * (1.0 - p.repetition) + 0.2 * p.rhythmic_variety + 0.2 * p.stepwise
            + 0.1 * p.coverage
            - 0.3 * p.chordal;
    }

    // Assign roles.
    let mut assigned = vec![false; n];
    for i in 0..n {
        let p = &mut profiles[i];
        if let Some(per) = p.period {
            if p.repetition >= 0.85 && p.note_count >= 4 * per {
                p.role = VoiceRole::Ostinato;
                assigned[i] = true;
            }
        }
    }
    if n >= 2 {
        let lowest = order[0];
        if !assigned[lowest] {
            profiles[lowest].role = VoiceRole::Bass;
            assigned[lowest] = true;
        }
    }
    for i in 0..n {
        let p = &mut profiles[i];
        if !assigned[i] && n >= 2 && (p.chordal > 0.5 || (p.repetition > 0.6 && p.density >= 2.0 && p.stepwise < 0.4)) {
            p.role = VoiceRole::Accompaniment;
            assigned[i] = true;
        }
    }
    let mut rest: Vec<usize> = (0..n).filter(|&i| !assigned[i]).collect();
    rest.sort_by(|&a, &b| profiles[b].salience.total_cmp(&profiles[a].salience));
    if let Some(&main) = rest.first() {
        profiles[main].role = VoiceRole::MainTheme;
        if let Some(&second) = rest.get(1) {
            let independent = rhythmic_independence(&raws[main].line, &raws[second].line);
            if profiles[second].salience >= 0.6 * profiles[main].salience && independent > 0.3 {
                profiles[second].role = VoiceRole::CounterMelody;
            }
        }
    } else if n == 1 {
        profiles[0].role = VoiceRole::MainTheme;
    }
    for p in &mut profiles {
        p.role_label = klang_i18n::translate(p.role.label_de());
    }

    // Cadences from bass and main voice.
    let mut cadences = Vec::new();
    if let Some(k) = key.filter(|_| with_cadences) {
        let bass = profiles.iter().position(|p| p.role == VoiceRole::Bass);
        let top = profiles.iter().position(|p| p.role == VoiceRole::MainTheme);
        if let Some(b) = bass {
            let line = &raws[b].line;
            for i in 1..line.len() {
                let (prev, cur) = (&line[i - 1], &line[i]);
                let is_end = i + 1 == line.len();
                let long = cur.duration >= 1.0 || is_end;
                if prev.midi % 12 == k.dominant() && cur.midi % 12 == k.tonic && long {
                    let on_tonic = top
                        .and_then(|t| sounding_at(&raws[t].line, cur.onset))
                        .map(|n| n.midi % 12 == k.tonic)
                        .unwrap_or(false);
                    let kind = if on_tonic { CadenceKind::Authentic } else { CadenceKind::ImperfectAuthentic };
                    cadences.push(Cadence {
                        onset: cur.onset,
                        kind,
                        label: if on_tonic { "Ganzschluss" } else { "unvollkommener Ganzschluss" },
                    });
                } else if cur.midi % 12 == k.dominant() && cur.duration >= 2.0 {
                    let gap_after = line.get(i + 1).map(|n| n.onset - (cur.onset + cur.duration)).unwrap_or(1.0);
                    if gap_after >= 0.0 && (is_end || gap_after > 0.0 || cur.duration >= 3.0) {
                        cadences.push(Cadence { onset: cur.onset, kind: CadenceKind::Half, label: "Halbschluss" });
                    }
                }
            }
        }
    }

    // Mood (valence/arousal) from mode, tempo, register, density.
    let tempo = score.tempo_bpm.unwrap_or(100.0) as f32;
    let all_notes = score.note_count().max(1) as f32;
    let avg_pitch = score.parts.iter().flat_map(|p| &p.voices).flat_map(|v| &v.notes).map(|n| n.midi as f32).sum::<f32>() / all_notes;
    let density = profiles.iter().map(|p| p.density).fold(0.0f32, f32::max);
    let mode = match key {
        Some(k) if k.minor => -1.0,
        Some(_) => 1.0,
        None => 0.0,
    };
    let valence = (0.6 * mode + 0.2 * ((avg_pitch - 64.0) / 12.0) + 0.2 * ((tempo - 100.0) / 60.0) - 0.3 * chromaticism).clamp(-1.0, 1.0);
    let arousal = (0.5 * ((tempo - 100.0) / 50.0) + 0.5 * ((density - 2.0) / 2.0)).clamp(-1.0, 1.0);
    // tid!: persisted as the "stimmung" facet in the score index and compared
    // with the order in `facets::DIMENSIONS`.
    let label = match (valence >= 0.0, arousal >= 0.0) {
        (true, true) => tid!("fröhlich · energisch"),
        (true, false) => tid!("heiter · ruhig"),
        (false, true) => tid!("dramatisch · aufgewühlt"),
        (false, false) => tid!("melancholisch · ruhig"),
    };

    ScoreAnalysis {
        key,
        key_name: key.map(|k| k.name_de()),
        tempo_bpm: score.tempo_bpm,
        duration_quarters: total,
        chromaticism,
        mood: Mood { valence, arousal, label },
        voices: profiles,
        cadences,
    }
}

/// Share of the onsets of `b` that do not coincide with onsets of `a`.
fn rhythmic_independence(a: &[MelodyNote], b: &[MelodyNote]) -> f32 {
    if b.is_empty() {
        return 0.0;
    }
    let mut j = 0;
    let mut shared = 0;
    for n in b {
        while j < a.len() && a[j].onset < n.onset - 1e-6 {
            j += 1;
        }
        if j < a.len() && (a[j].onset - n.onset).abs() < 1e-6 {
            shared += 1;
        }
    }
    1.0 - shared as f32 / b.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::parse_musicxml;
    use crate::score::tests::make_xml;

    #[test]
    fn finds_theme_ostinato_bass_and_cadence() {
        // Melody in C major (eighth = 1), ends on c''.
        let melody: &[(u8, u32)] = &[
            (72, 2), (74, 1), (76, 1), (77, 2), (76, 2), (74, 3), (72, 1), (71, 2), (74, 2),
            (76, 2), (79, 2), (77, 1), (76, 1), (74, 2), (72, 4), (71, 2), (74, 2), (72, 8),
        ];
        // Ostinato: the same eighth-note figure over and over.
        let fig = [(60u8, 1u32), (64, 1), (67, 1), (64, 1)];
        let ostinato: Vec<(u8, u32)> = fig.iter().copied().cycle().take(40).collect();
        // Bass in long values, G → C at the end.
        let bass: &[(u8, u32)] = &[(48, 8), (53, 8), (55, 8), (43, 8), (48, 8)];
        let xml = make_xml("Probe", &[("Flöte", melody), ("Harfe", &ostinato), ("Cello", bass)]);
        let score = parse_musicxml(&xml).unwrap();
        let a = analyze_score(&score);

        let role = |name: &str| a.voices.iter().find(|v| v.part == name).unwrap().role;
        assert_eq!(role("Flöte"), VoiceRole::MainTheme, "{:#?}", a.voices);
        assert_eq!(role("Harfe"), VoiceRole::Ostinato, "{:#?}", a.voices);
        assert_eq!(role("Cello"), VoiceRole::Bass, "{:#?}", a.voices);
        assert_eq!(a.key_name.as_deref(), Some("C-Dur"));
        assert!(a.cadences.iter().any(|c| c.kind == CadenceKind::Authentic && (c.onset - 16.0).abs() < 1e-6), "{:?}", a.cadences);
        assert!(a.mood.valence > 0.0);
    }
}
