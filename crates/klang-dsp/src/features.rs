//! Messbare Klangeigenschaften – die „Ohren“ des Agenten.
//!
//! Aus einem Klang werden Zahlen, die mit Wörtern wie hell, warm, hart oder
//! lang ausklingend zusammenhängen. So kann ein Sprachmodell prüfen, ob eine
//! Änderung die gewünschte Wirkung hatte.

use crate::fft::{hann, Complex, Fft};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFeatures {
    /// Spektraler Schwerpunkt in Hz (energiegewichtet) – höher = heller.
    pub centroid_hz: f32,
    /// Unterhalb dieser Frequenz liegen 85 % der Energie.
    pub rolloff_hz: f32,
    /// 0 = rein tonal, 1 = rauschartig.
    pub flatness: f32,
    /// Energieanteil 60–400 Hz – Wärme, Körper.
    pub low_share: f32,
    /// Energieanteil über 3 kHz – Brillanz, Schärfe.
    pub high_share: f32,
    /// Anstiegszeit 10 % → 90 % des Spitzenpegels in ms.
    pub attack_ms: f32,
    /// Spektraler Schwerpunkt der ersten 40 ms – Härte des Anschlags.
    pub onset_centroid_hz: f32,
    /// Geschätzte Ausklingzeit T60 in Sekunden (aus dem −20-dB-Abfall).
    pub decay_s: f32,
    pub peak: f32,
    pub rms: f32,
}

fn spectrum_stats(power: &[f32], bin_hz: f32) -> (f32, f32, f32, f32, f32) {
    let total: f32 = power.iter().skip(1).sum::<f32>().max(1e-20);
    let mut num = 0.0;
    let (mut low, mut high) = (0.0, 0.0);
    let mut log_sum = 0.0f64;
    for (k, &p) in power.iter().enumerate().skip(1) {
        let f = k as f32 * bin_hz;
        num += f * p;
        if (60.0..400.0).contains(&f) {
            low += p;
        }
        if f >= 3000.0 {
            high += p;
        }
        log_sum += ((p + 1e-20) as f64).ln();
    }
    let n = (power.len() - 1).max(1) as f64;
    let flatness = ((log_sum / n).exp() / (total as f64 / n)) as f32;
    let mut acc = 0.0;
    let mut rolloff = 0.0;
    for (k, &p) in power.iter().enumerate().skip(1) {
        acc += p;
        if acc >= 0.85 * total {
            rolloff = k as f32 * bin_hz;
            break;
        }
    }
    (num / total, rolloff, flatness.clamp(0.0, 1.0), low / total, high / total)
}

fn power_spectrum(fft: &Fft, win: &[f32], x: &[f32], start: usize, acc: &mut [f32]) {
    let n = fft.size();
    let mut buf: Vec<Complex> = (0..n)
        .map(|i| Complex::new(x.get(start + i).copied().unwrap_or(0.0) * win[i], 0.0))
        .collect();
    fft.forward(&mut buf);
    for (k, a) in acc.iter_mut().enumerate() {
        *a += buf[k].norm_sqr();
    }
}

pub fn analyze_sound(x: &[f32], sr: f32) -> SoundFeatures {
    if x.is_empty() {
        return SoundFeatures::default();
    }
    // Hüllkurve in 2-ms-Blöcken.
    let block = ((sr * 0.002) as usize).max(1);
    let env: Vec<f32> = x
        .chunks(block)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let (peak_i, peak_env) = env
        .iter()
        .copied()
        .enumerate()
        .fold((0, 0.0f32), |a, (i, v)| if v > a.1 { (i, v) } else { a });
    let block_s = block as f32 / sr;
    let t10 = env.iter().position(|&v| v >= 0.1 * peak_env).unwrap_or(0);
    let t90 = env.iter().position(|&v| v >= 0.9 * peak_env).unwrap_or(peak_i);
    let attack_ms = (t90.saturating_sub(t10) as f32 * block_s * 1000.0).max(block_s * 1000.0 * 0.5);

    let decay_s = match env[peak_i..].iter().position(|&v| v < 0.1 * peak_env) {
        Some(k) => k as f32 * block_s * 3.0,
        None => {
            let end = env.last().copied().unwrap_or(0.0).max(1e-9);
            let drop_db = 20.0 * (end / peak_env.max(1e-9)).log10();
            let span = (env.len() - peak_i) as f32 * block_s;
            if drop_db < -1.0 {
                (span * 60.0 / -drop_db).min(30.0)
            } else {
                30.0
            }
        }
    };

    let n = 4096usize;
    let fft = Fft::new(n);
    let win = hann(n);
    let bins = n / 2 + 1;
    let bin_hz = sr / n as f32;
    let mut acc = vec![0f32; bins];
    // Nur vollständige Fenster ab dem Pegelmaximum (max. 1 s), sonst verschmiert
    // der harte Abbruch am Signalende das Spektrum.
    let from = (peak_i * block).min(x.len().saturating_sub(n));
    let until = (from + sr as usize).min(x.len());
    let mut s = from;
    loop {
        power_spectrum(&fft, &win, x, s, &mut acc);
        s += n / 2;
        if s + n > until {
            break;
        }
    }
    let (centroid_hz, rolloff_hz, flatness, low_share, high_share) = spectrum_stats(&acc, bin_hz);

    let short = 2048usize;
    let fft_s = Fft::new(short);
    let win_s = hann(short);
    let mut onset = vec![0f32; short / 2 + 1];
    power_spectrum(&fft_s, &win_s, x, t10 * block, &mut onset);
    let (onset_centroid_hz, ..) = spectrum_stats(&onset, sr / short as f32);

    SoundFeatures {
        centroid_hz,
        rolloff_hz,
        flatness,
        low_share,
        high_share,
        attack_ms,
        onset_centroid_hz,
        decay_s,
        peak: x.iter().fold(0.0f32, |m, v| m.max(v.abs())),
        rms: (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{pluck, PluckParams};
    use crate::testutil::sine;

    const SR: f32 = 48_000.0;

    fn tone(p: PluckParams) -> SoundFeatures {
        analyze_sound(&pluck(220.0, 2.5, 1.0, SR, &p, 7), SR)
    }

    #[test]
    #[ignore]
    fn print_calibration_grid() {
        let base = PluckParams { brightness: 0.5, decay: 0.5, body: 0.5, attack: 0.5 };
        for v in [0.0f32, 0.05, 0.25, 0.5, 0.75, 0.95, 1.0] {
            let b = tone(PluckParams { brightness: v, ..base });
            let d = tone(PluckParams { decay: v, ..base });
            let w = tone(PluckParams { body: v, ..base });
            let a = tone(PluckParams { attack: v, ..base });
            eprintln!(
                "v={v:.2}  hell: rel={:.2}  abkl: {:.2}s  körper: low={:.3}  anschlag: rel={:.2}",
                b.centroid_hz / 220.0, d.decay_s, w.low_share, a.onset_centroid_hz / 220.0
            );
        }
    }

    #[test]
    fn sine_is_tonal_and_centered() {
        let f = analyze_sound(&sine(1000.0, 1.0, SR, 0.5), SR);
        assert!((f.centroid_hz - 1000.0).abs() < 30.0, "{f:?}");
        assert!(f.flatness < 0.05, "{f:?}");
    }

    #[test]
    fn noise_is_flat() {
        let mut seed = 99u32;
        let x: Vec<f32> = (0..48_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        assert!(analyze_sound(&x, SR).flatness > 0.4);
    }

    #[test]
    fn parameters_move_the_matching_feature() {
        let base = PluckParams { brightness: 0.5, decay: 0.5, body: 0.4, attack: 0.5 };
        let dark = tone(PluckParams { brightness: 0.1, ..base });
        let bright = tone(PluckParams { brightness: 0.9, ..base });
        assert!(bright.centroid_hz > 1.3 * dark.centroid_hz, "{dark:?} / {bright:?}");

        let short = tone(PluckParams { decay: 0.1, ..base });
        let long = tone(PluckParams { decay: 0.9, ..base });
        assert!(long.decay_s > 2.0 * short.decay_s, "{short:?} / {long:?}");

        let thin = tone(PluckParams { body: 0.05, ..base });
        let warm = tone(PluckParams { body: 0.95, ..base });
        assert!(warm.low_share > thin.low_share, "{thin:?} / {warm:?}");

        let soft = tone(PluckParams { attack: 0.05, ..base });
        let hard = tone(PluckParams { attack: 0.95, ..base });
        assert!(hard.onset_centroid_hz > soft.onset_centroid_hz, "{soft:?} / {hard:?}");
    }
}
