//! Eigene komplexe Radix-2-FFT (in-place, iterativ).

use std::f64::consts::PI;
use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

impl Complex {
    pub const ZERO: Complex = Complex { re: 0.0, im: 0.0 };

    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }
    pub fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
    pub fn norm_sqr(self) -> f32 {
        self.re * self.re + self.im * self.im
    }
    pub fn norm(self) -> f32 {
        self.norm_sqr().sqrt()
    }
}

impl Add for Complex {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for Complex {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for Complex {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}

/// Vorberechnete FFT fester Größe (Zweierpotenz).
pub struct Fft {
    n: usize,
    twiddles: Vec<Complex>,
    bitrev: Vec<u32>,
}

impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(
            n >= 2 && n.is_power_of_two(),
            "FFT-Größe muss eine Zweierpotenz >= 2 sein"
        );
        let bits = n.trailing_zeros();
        let bitrev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                Complex::new(a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Self { n, twiddles, bitrev }
    }

    pub fn size(&self) -> usize {
        self.n
    }

    pub fn forward(&self, buf: &mut [Complex]) {
        self.transform(buf, false);
    }

    /// Inverse Transformation inklusive 1/N-Skalierung.
    pub fn inverse(&self, buf: &mut [Complex]) {
        self.transform(buf, true);
        let s = 1.0 / self.n as f32;
        for c in buf.iter_mut() {
            c.re *= s;
            c.im *= s;
        }
    }

    fn transform(&self, buf: &mut [Complex], inverse: bool) {
        assert_eq!(buf.len(), self.n, "Puffergröße passt nicht zur FFT");
        for i in 0..self.n {
            let j = self.bitrev[i] as usize;
            if j > i {
                buf.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= self.n {
            let half = size / 2;
            let step = self.n / size;
            for start in (0..self.n).step_by(size) {
                for k in 0..half {
                    let mut w = self.twiddles[k * step];
                    if inverse {
                        w.im = -w.im;
                    }
                    let a = buf[start + k];
                    let b = buf[start + k + half] * w;
                    buf[start + k] = a + b;
                    buf[start + k + half] = a - b;
                }
            }
            size <<= 1;
        }
    }
}

/// Periodisches Hann-Fenster (summiert sich bei 50 % Überlappung zu 1).
pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_restores_signal() {
        let fft = Fft::new(64);
        let orig: Vec<Complex> = (0..64)
            .map(|i| Complex::new((i as f32 * 0.3).sin(), (i as f32 * 0.1).cos()))
            .collect();
        let mut buf = orig.clone();
        fft.forward(&mut buf);
        fft.inverse(&mut buf);
        for (a, b) in orig.iter().zip(&buf) {
            assert!((a.re - b.re).abs() < 1e-4 && (a.im - b.im).abs() < 1e-4);
        }
    }

    #[test]
    fn cosine_lands_in_its_bin() {
        let n = 256;
        let fft = Fft::new(n);
        let mut buf: Vec<Complex> = (0..n)
            .map(|i| {
                Complex::new(
                    (2.0 * std::f32::consts::PI * 10.0 * i as f32 / n as f32).cos(),
                    0.0,
                )
            })
            .collect();
        fft.forward(&mut buf);
        let peak = (0..n / 2)
            .max_by(|&a, &b| buf[a].norm().total_cmp(&buf[b].norm()))
            .unwrap();
        assert_eq!(peak, 10);
        assert!((buf[10].norm() - n as f32 / 2.0).abs() < 1e-2);
    }
}
