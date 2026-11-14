//! Stereo-Hall für den Raum-Anteil der Spuren (nach dem bekannten Aufbau aus
//! acht parallelen Kammfiltern mit gedämpfter Rückkopplung und vier Allpässen
//! je Kanal; der rechte Kanal ist leicht verstimmt, das macht die Breite).
//!
//! Alles wird beim Anlegen reserviert – `process` allokiert nichts und darf im
//! Audio-Callback laufen.

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
/// Versatz des rechten Kanals (Samples bei 44,1 kHz).
const SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0, store: 0.0 }
    }

    #[inline]
    fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.pos];
        self.store = out * (1.0 - damp) + self.store * damp;
        // sehr kleine Werte nullen: denormale Zahlen bremsen die CPU im Ausklang
        if self.store.abs() < 1e-18 {
            self.store = 0.0;
        }
        self.buf[self.pos] = x + self.store * feedback;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        out
    }
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0 }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        let out = b - x;
        let v = x + b * 0.5;
        self.buf[self.pos] = if v.abs() < 1e-18 { 0.0 } else { v };
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        out
    }
}

pub struct Reverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    predelay: Vec<f32>,
    pre_pos: usize,
    /// Tiefen vor dem Hall weg (sonst dröhnt er): einpoliger Hochpass.
    hp_lp: f32,
    hp_coef: f32,
    feedback: f32,
    damp: f32,
}

impl Reverb {
    /// Mittelgroßer, warmer Saal für die Abtastrate `sr`.
    pub fn new(sr: f32) -> Self {
        let scale = sr / 44_100.0;
        let len = |n: usize, extra: usize| ((n + extra) as f32 * scale).round() as usize;
        let side = |extra: usize| -> (Vec<Comb>, Vec<Allpass>) {
            (COMBS.iter().map(|&n| Comb::new(len(n, extra))).collect(), ALLPASSES.iter().map(|&n| Allpass::new(len(n, extra))).collect())
        };
        let (cl, al) = side(0);
        let (cr, ar) = side(SPREAD);
        Self {
            combs: [cl, cr],
            allpasses: [al, ar],
            predelay: vec![0.0; ((0.018 * sr) as usize).max(1)],
            pre_pos: 0,
            hp_lp: 0.0,
            hp_coef: 1.0 - (-2.0 * std::f32::consts::PI * 160.0 / sr).exp(),
            feedback: 0.84,
            damp: 0.32,
        }
    }

    /// Ein Sample Hall-Eingang (Summe der Sends) → Hall links/rechts.
    #[inline]
    pub fn process(&mut self, input: f32) -> (f32, f32) {
        let delayed = self.predelay[self.pre_pos];
        self.predelay[self.pre_pos] = input;
        self.pre_pos += 1;
        if self.pre_pos == self.predelay.len() {
            self.pre_pos = 0;
        }
        self.hp_lp += self.hp_coef * (delayed - self.hp_lp);
        if self.hp_lp.abs() < 1e-18 {
            self.hp_lp = 0.0;
        }
        let x = (delayed - self.hp_lp) * 0.015;
        let mut out = [0f32; 2];
        for (ch, o) in out.iter_mut().enumerate() {
            let mut s = 0.0;
            for c in self.combs[ch].iter_mut() {
                s += c.process(x, self.feedback, self.damp);
            }
            for a in self.allpasses[ch].iter_mut() {
                s = a.process(s);
            }
            *o = s;
        }
        (out[0], out[1])
    }
}

/// Weicher Begrenzer für die Summe: bis 0,7 unverändert, darüber sanft gegen 1
/// (statt hart abzuschneiden – das klingt bei vielen Stimmen kratzig).
#[inline]
pub fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.7;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let y = KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh();
        y.copysign(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_rings_out_in_both_channels_and_decays() {
        let sr = 48_000.0;
        let mut r = Reverb::new(sr);
        let mut l = Vec::new();
        let mut rr = Vec::new();
        for i in 0..(3.0 * sr) as usize {
            let (a, b) = r.process(if i == 0 { 1.0 } else { 0.0 });
            l.push(a);
            rr.push(b);
        }
        let energy = |x: &[f32], a: f32, b: f32| x[(a * sr) as usize..(b * sr) as usize].iter().map(|v| v * v).sum::<f32>();
        assert!(energy(&l, 0.0, 0.3) > 0.0 && energy(&rr, 0.0, 0.3) > 0.0, "klingt auf beiden Seiten");
        assert!(energy(&l, 2.5, 3.0) < energy(&l, 0.05, 0.55) * 0.01, "klingt aus");
        assert!(l != rr, "links und rechts verschieden (Breite)");
        assert!(l[..(0.015 * sr) as usize].iter().all(|v| *v == 0.0), "Vorverzögerung");
    }

    #[test]
    fn soft_limit_is_transparent_below_the_knee_and_never_clips() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.7), -0.7);
        assert!(soft_limit(3.0) < 1.0 && soft_limit(3.0) > 0.95);
        assert!(soft_limit(-3.0) > -1.0);
        assert!(soft_limit(0.8) > 0.75 && soft_limit(0.8) < 0.8);
    }
}
