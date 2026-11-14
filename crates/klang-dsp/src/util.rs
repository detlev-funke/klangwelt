//! Small helpers around levels and statistics.

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-9).log10()
}

/// Median; sorts the buffer passed in. Empty buffer → 0.
pub fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let m = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[m - 1] + values[m]) * 0.5
    } else {
        values[m]
    }
}

/// Scales so that the peak reaches `target_peak`.
pub fn normalize(x: &mut [f32], target_peak: f32) {
    let p = peak(x);
    if p > 1e-9 {
        let g = target_peak / p;
        for v in x.iter_mut() {
            *v *= g;
        }
    }
}

/// Waveform overview for display: `buckets` pairs of (min, max).
pub fn overview(samples: &[f32], buckets: usize) -> Vec<f32> {
    let buckets = buckets.max(1);
    if samples.is_empty() {
        return vec![0.0; buckets * 2];
    }
    let len = samples.len();
    let mut out = Vec::with_capacity(buckets * 2);
    for b in 0..buckets {
        let s = b * len / buckets;
        let e = ((b + 1) * len / buckets).max(s + 1).min(len);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &v in &samples[s..e] {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        out.push(lo);
        out.push(hi);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_even_and_odd() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
        assert_eq!(median(&mut []), 0.0);
    }

    #[test]
    fn db_roundtrip() {
        assert!((gain_to_db(db_to_gain(-6.0)) + 6.0).abs() < 1e-4);
    }

    #[test]
    fn overview_has_pairs() {
        let o = overview(&[0.0, 1.0, -1.0, 0.5], 2);
        assert_eq!(o, vec![0.0, 1.0, -1.0, 0.5]);
        assert_eq!(overview(&[0.1], 4).len(), 8);
    }
}
