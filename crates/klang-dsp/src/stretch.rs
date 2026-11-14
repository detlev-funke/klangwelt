//! Tempo und Tonhöhe unabhängig voneinander ändern.
//!
//! * Zeitdehnung: WSOLA (Waveform Similarity Overlap-Add). Jeder neue Frame
//!   wird so gewählt, dass er möglichst nahtlos an den vorherigen anschließt.
//! * Resampling: gefenstertes Sinc mit Tiefpass bei Abwärtswandlung.
//! * Tonhöhe: dehnen um den Faktor 2^(n/12), dann zurück-resamplen.

use crate::fft::hann;

#[inline]
fn sample(x: &[f32], i: isize) -> f32 {
    if i >= 0 && (i as usize) < x.len() {
        x[i as usize]
    } else {
        0.0
    }
}

/// Ändert die Dauer um `ratio` (Ausgabelänge / Eingabelänge), Tonhöhe bleibt.
pub fn time_stretch(input: &[f32], ratio: f64, sample_rate: f32) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    let ratio = ratio.clamp(0.1, 10.0);
    if (ratio - 1.0).abs() < 1e-9 {
        return input.to_vec();
    }
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let n = (((sample_rate * 0.046) as usize).max(64)) & !1;
    let hs = n / 2;
    let tol = (hs / 2) as isize;
    let win = hann(n);

    let mut out = vec![0f32; out_len + n];
    let mut norm = vec![0f32; out_len + n];
    let mut prev_pos: isize = 0;
    let mut k = 0usize;
    loop {
        let out_pos = k * hs;
        if out_pos >= out_len {
            break;
        }
        let nominal = (out_pos as f64 / ratio).round() as isize;
        let pos = if k == 0 {
            0
        } else {
            best_offset(input, nominal, prev_pos + hs as isize, hs, tol)
        };
        for j in 0..n {
            let w = win[j];
            out[out_pos + j] += sample(input, pos + j as isize) * w;
            norm[out_pos + j] += w;
        }
        prev_pos = pos;
        k += 1;
    }
    for i in 0..out_len {
        if norm[i] > 1e-3 {
            out[i] /= norm[i];
        }
    }
    out.truncate(out_len);
    out
}

/// Korrelation von `x[a..]` und `x[b..]` über `hs` Samples (jedes
/// `stride`-te). Liegen beide ganz im Signal (der Normalfall), ohne
/// Grenzprüfung je Sample – gleiche Summenreihenfolge, gleiches Ergebnis wie
/// [`correlate_edge`].
fn correlate(x: &[f32], a: isize, b: isize, hs: usize, stride: usize) -> f32 {
    let inside = |p: isize| p >= 0 && p as usize + hs <= x.len();
    if inside(a) && inside(b) {
        let (u, v) = (&x[a as usize..][..hs], &x[b as usize..][..hs]);
        let mut s = 0.0;
        let mut j = 0;
        while j < hs {
            s += u[j] * v[j];
            j += stride;
        }
        return s;
    }
    correlate_edge(x, a, b, hs, stride)
}

/// Wie [`correlate`], außerhalb des Signals mit Nullen.
fn correlate_edge(x: &[f32], a: isize, b: isize, hs: usize, stride: usize) -> f32 {
    let mut s = 0.0;
    let mut j = 0;
    while j < hs {
        s += sample(x, a + j as isize) * sample(x, b + j as isize);
        j += stride;
    }
    s
}

/// Sucht um `nominal` die Position, deren Anfang am besten zur natürlichen
/// Fortsetzung `natural` des vorherigen Frames passt (grob, dann fein).
fn best_offset(x: &[f32], nominal: isize, natural: isize, hs: usize, tol: isize) -> isize {
    let corr = |cand: isize, stride: usize| correlate(x, cand, natural, hs, stride);
    let mut best = nominal;
    let mut best_s = corr(nominal, 4);
    let mut d = -tol;
    while d <= tol {
        let s = corr(nominal + d, 4);
        if s > best_s {
            best_s = s;
            best = nominal + d;
        }
        d += 4;
    }
    let coarse = best;
    let mut best_s = corr(coarse, 2);
    for d in -4..=4 {
        let c = coarse + d;
        if (c - nominal).abs() > tol {
            continue;
        }
        let s = corr(c, 2);
        if s > best_s {
            best_s = s;
            best = c;
        }
    }
    best
}

/// Ab so vielen Ausgabe-Samples je Kern lohnt es sich, das Resampling zu
/// verteilen (Desktop; im Browser gibt es keine Threads).
#[cfg(not(target_arch = "wasm32"))]
const RESAMPLE_PER_THREAD: usize = 48_000;

/// Resampling um `factor` = Ausgaberate / Eingaberate (bzw. Ausgabelänge / Eingabelänge).
/// Große Puffer werden auf dem Desktop auf mehrere Kerne verteilt; jedes
/// Ausgabe-Sample wird dabei genau gleich gerechnet.
pub fn resample(input: &[f32], factor: f64) -> Vec<f32> {
    if input.is_empty() || factor <= 0.0 {
        return Vec::new();
    }
    if (factor - 1.0).abs() < 1e-12 {
        return input.to_vec();
    }
    let out_len = ((input.len() as f64) * factor).round() as usize;
    let kernel = SincKernel::new(factor);
    let mut out = vec![0f32; out_len];
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let parts = cores.min(8).min(out_len / RESAMPLE_PER_THREAD).max(1);
        if parts > 1 {
            let chunk = out_len.div_ceil(parts);
            std::thread::scope(|s| {
                for (p, part) in out.chunks_mut(chunk).enumerate() {
                    let kernel = &kernel;
                    s.spawn(move || kernel.fill(input, p * chunk, part));
                }
            });
            return out;
        }
    }
    kernel.fill(input, 0, &mut out);
    out
}

/// Gefenstertes Sinc (Hann) für das Resampling um einen festen Faktor.
struct SincKernel {
    factor: f64,
    cutoff: f64,
    half: isize,
    /// Drehung je Filterpunkt: Sinc-Winkel (π·cutoff) und Fensterwinkel (π/half).
    rot_sinc: (f64, f64),
    rot_win: (f64, f64),
}

impl SincKernel {
    fn new(factor: f64) -> Self {
        use std::f64::consts::PI;
        let cutoff = factor.min(1.0);
        let half = (12.0 / cutoff).ceil() as isize;
        Self { factor, cutoff, half, rot_sinc: (PI * cutoff).sin_cos(), rot_win: (PI / half as f64).sin_cos() }
    }

    /// Ausgabe-Samples ab Index `first_out` in `out`.
    fn fill(&self, input: &[f32], first_out: usize, out: &mut [f32]) {
        use std::f64::consts::PI;
        let (cutoff, half) = (self.cutoff, self.half);
        let hf = half as f64;
        let (sin_c, cos_c) = self.rot_sinc;
        let (sin_w, cos_w) = self.rot_win;
        for (o, y) in out.iter_mut().enumerate() {
            let x = (first_out + o) as f64 / self.factor;
            let center = x.floor() as isize;
            let first = center - half + 1;
            let d0 = x - first as f64;
            // Von Filterpunkt zu Filterpunkt sinkt der Abstand d um genau 1:
            // Sinc- und Fensterwinkel drehen sich um feste Schritte weiter,
            // statt für jeden Punkt neu sin/cos zu rechnen (je Ausgabe-Sample
            // 2 statt 4·half). sin(π·cutoff·d) für den Sinc, cos(π·d/half) für das Fenster.
            let (mut s, mut c) = (PI * cutoff * d0).sin_cos();
            let (mut ws, mut wc) = (PI * d0 / hf).sin_cos();
            let mut acc = 0.0f64;
            let mut wsum = 0.0f64;
            for k in first..=(center + half) {
                let d = x - k as f64;
                if d.abs() < hf {
                    let arg = d * cutoff;
                    let sinc = if arg.abs() < 1e-9 { 1.0 } else { s / (PI * arg) };
                    let w = 0.5 + 0.5 * wc;
                    let coef = sinc * w;
                    acc += sample(input, k) as f64 * coef;
                    wsum += coef;
                }
                // d → d − 1
                (s, c) = (s * cos_c - c * sin_c, c * cos_c + s * sin_c);
                (ws, wc) = (ws * cos_w - wc * sin_w, wc * cos_w + ws * sin_w);
            }
            *y = if wsum.abs() > 1e-9 { (acc / wsum) as f32 } else { 0.0 };
        }
    }
}

/// Tonhöhe um `semitones` verschieben, Dauer bleibt gleich.
pub fn pitch_shift(input: &[f32], semitones: f64, sample_rate: f32) -> Vec<f32> {
    render_warp(input, sample_rate, 1.0, semitones, 1.0)
}

/// Alles in einem Durchgang: Dauer × `time_ratio`, Tonhöhe + `semitones`,
/// Abtastrate × `rate_factor` (z. B. 48000/44100).
pub fn render_warp(
    input: &[f32],
    sample_rate: f32,
    time_ratio: f64,
    semitones: f64,
    rate_factor: f64,
) -> Vec<f32> {
    let pf = 2f64.powf(semitones / 12.0);
    let stretch = time_ratio * pf;
    let stretched = if (stretch - 1.0).abs() < 1e-6 {
        input.to_vec()
    } else {
        time_stretch(input, stretch, sample_rate)
    };
    let factor = rate_factor / pf;
    if (factor - 1.0).abs() < 1e-9 {
        stretched
    } else {
        resample(&stretched, factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{detect_pitch, YinConfig};
    use crate::testutil::sine;
    use crate::util::median;

    fn pitch_of(x: &[f32], sr: f32) -> f32 {
        let mut f: Vec<f32> = detect_pitch(x, sr, &YinConfig::default())
            .iter()
            .filter_map(|f| f.freq)
            .collect();
        median(&mut f)
    }

    #[test]
    fn stretch_changes_length_not_pitch() {
        let sr = 44_100.0;
        let x = sine(330.0, 1.0, sr, 0.5);
        for &r in &[0.5, 0.8, 1.25, 2.0] {
            let y = time_stretch(&x, r, sr);
            assert_eq!(y.len(), (x.len() as f64 * r).round() as usize);
            let p = pitch_of(&y, sr);
            assert!((p - 330.0).abs() < 3.0, "ratio {r}: {p} Hz");
        }
    }

    #[test]
    fn pitch_shift_octave_up() {
        let sr = 44_100.0;
        let x = sine(220.0, 1.0, sr, 0.5);
        let y = pitch_shift(&x, 12.0, sr);
        assert!((y.len() as i64 - x.len() as i64).abs() <= 2);
        let p = pitch_of(&y, sr);
        assert!((p - 440.0).abs() < 4.0, "{p}");
    }

    #[test]
    fn resample_keeps_frequency_in_seconds() {
        let x = sine(440.0, 0.5, 44_100.0, 0.5);
        let y = resample(&x, 48_000.0 / 44_100.0);
        assert!((y.len() as i64 - 24_000).abs() <= 1);
        let p = pitch_of(&y, 48_000.0);
        assert!((p - 440.0).abs() < 2.0, "{p}");
    }

    /// Die bisherige, direkte Rechnung (sin/cos an jedem Filterpunkt) als Maßstab.
    fn resample_direct(input: &[f32], factor: f64) -> Vec<f32> {
        let out_len = ((input.len() as f64) * factor).round() as usize;
        let cutoff = factor.min(1.0);
        let half = (12.0 / cutoff).ceil() as isize;
        (0..out_len)
            .map(|i| {
                let x = i as f64 / factor;
                let center = x.floor() as isize;
                let (mut acc, mut wsum) = (0.0f64, 0.0f64);
                for k in (center - half + 1)..=(center + half) {
                    let d = x - k as f64;
                    if d.abs() >= half as f64 {
                        continue;
                    }
                    let arg = d * cutoff;
                    let p = std::f64::consts::PI * arg;
                    let sinc = if arg.abs() < 1e-9 { 1.0 } else { p.sin() / p };
                    let c = sinc * (0.5 + 0.5 * (std::f64::consts::PI * d / half as f64).cos());
                    acc += sample(input, k) as f64 * c;
                    wsum += c;
                }
                if wsum.abs() > 1e-9 { (acc / wsum) as f32 } else { 0.0 }
            })
            .collect()
    }

    #[test]
    fn resample_matches_the_direct_formula() {
        // Rauschen plus Ton: alle Frequenzen, auch die, die der Tiefpass dämpft
        let mut seed = 7u32;
        let x: Vec<f32> = sine(523.0, 0.3, 48_000.0, 0.4)
            .iter()
            .map(|v| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                v + ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.4
            })
            .collect();
        // 48→44,1 kHz, 44,1→48 kHz, Tonhöhen-Faktoren, Oktaven
        for factor in [44_100.0 / 48_000.0, 48_000.0 / 44_100.0, 2f64.powf(-3.0 / 12.0), 2f64.powf(7.0 / 12.0), 0.5, 2.0, 0.3] {
            let a = resample(&x, factor);
            let b = resample_direct(&x, factor);
            assert_eq!(a.len(), b.len());
            let diff = a.iter().zip(&b).map(|(p, q)| (p - q).abs()).fold(0f32, f32::max);
            assert!(diff < 1e-6, "Faktor {factor}: größte Abweichung {diff}");
        }
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 9) as f32 / (1u32 << 23) as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn resample_on_several_cores_matches_one_core_exactly() {
        let x = noise(48_000 * 5, 11);
        for factor in [44_100.0 / 48_000.0, 48_000.0 / 44_100.0, 2f64.powf(-5.0 / 12.0)] {
            let parallel = resample(&x, factor);
            let mut serial = vec![0f32; parallel.len()];
            SincKernel::new(factor).fill(&x, 0, &mut serial);
            assert!(parallel == serial, "Faktor {factor}: verteilt anders als am Stück");
        }
    }

    #[test]
    fn fast_correlation_matches_the_edge_safe_one_exactly() {
        let x = noise(10_000, 3);
        let hs = 1_104;
        for (a, b) in [(0isize, 0isize), (17, 4_000), (8_896, 5), (3_333, 6_001)] {
            for stride in [1, 2, 4] {
                assert_eq!(correlate(&x, a, b, hs, stride).to_bits(), correlate_edge(&x, a, b, hs, stride).to_bits(), "a={a} b={b} stride={stride}");
            }
        }
        // am Rand: mit Nullen aufgefüllt
        assert_eq!(correlate(&x, -10, 0, hs, 2), correlate_edge(&x, -10, 0, hs, 2));
        assert_eq!(correlate(&x, 9_500, 0, hs, 2), correlate_edge(&x, 9_500, 0, hs, 2));
    }
}
