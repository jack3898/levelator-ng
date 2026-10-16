//! Step 1: `highpass` (0x3e48c → 0x3d836).
//!
//! `--highpass__low 25 --highpass__high 100` do not set two cutoffs: they
//! define one RBJ-cookbook 2nd-order high-pass biquad centred on their mean
//! (62.5 Hz) with a bandwidth of their octave span (2 octaves). Output is
//! clamped to [-1, 1]. The stage also returns the dB RMS level of the
//! filtered signal, which the pipeline uses to set the gate threshold.

use super::{Progress, Signal};
use crate::Error;

#[derive(Clone, Debug)]
pub struct HighpassArgs {
    pub low_hz: f64,
    pub high_hz: f64,
}

impl Default for HighpassArgs {
    fn default() -> Self {
        HighpassArgs {
            low_hz: 25.0,
            high_hz: 100.0,
        }
    }
}

/// Direct Form I biquad with the original's evaluation order (0x3c364).
#[derive(Clone, Debug)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// RBJ high-pass (0x3c3da, type 1) with bandwidth in octaves.
    pub fn highpass(f0: f64, fs: f64, bw: f64) -> Self {
        let w0 = (f0 * std::f64::consts::TAU) / fs;
        let s = w0.sin();
        let c = w0.cos();
        let alpha = ((w0 * (bw * 0.34657359027997264)) / s).sinh() * s;
        let b0 = (c + 1.0) * 0.5;
        let b1 = -(c + 1.0);
        let a0 = alpha + 1.0;
        let a1 = c * -2.0;
        let a2 = 1.0 - alpha;
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b0 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    pub fn tick(&mut self, x: f64) -> f64 {
        let y = (((x * self.b0 + self.x1 * self.b1) + self.b2 * self.x2) - self.a1 * self.y1)
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn clamp1(y: f64) -> f64 {
    let t = if -1.0 > y { -1.0 } else { y };
    if !(1.0 < y) {
        t
    } else {
        1.0
    }
}

/// Returns the filtered signal and its level in dB (0.0 for silence).
pub fn run(
    sig: Signal,
    args: &HighpassArgs,
    progress: &mut Progress,
    name: &str,
) -> Result<(Signal, f64), Error> {
    let ch = sig.channels;
    if ch > 2 {
        return Err(Error::Format(format!(
            "{name}: cannot process more than 2 channels;  has {ch}"
        )));
    }
    let center = (args.low_hz + args.high_hz) * 0.5;
    let bw = (args.high_hz / args.low_hz).ln() / 2f64.ln();
    let mut filters = vec![Biquad::highpass(center, sig.sample_rate as f64, bw); ch];
    let mut sums = vec![0.0f64; ch];
    let frames = sig.frames();
    let mut out = sig;
    let block = 8192 - 8192 % ch;
    let nblocks = out.data.len().div_ceil(block);
    for (bi, chunk) in out.data.chunks_mut(block).enumerate() {
        for frame in chunk.chunks_mut(ch) {
            for (c, x) in frame.iter_mut().enumerate() {
                let y = clamp1(filters[c].tick(*x));
                *x = y;
                sums[c] = y * y + sums[c];
            }
        }
        progress.update(0.0, 14.0, Progress::frac(bi, nblocks))?;
    }
    let (l, r) = (sums[0], sums.get(1).copied().unwrap_or(0.0));
    let m = if r <= l { l } else { r };
    let level = if m == 0.0 {
        0.0
    } else {
        20.0 * (m / frames as f64).sqrt().log10()
    };
    out.requantize();
    Ok((out, level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_dc_and_passes_speech_band() {
        let mut bq = Biquad::highpass(62.5, 44100.0, 2.0);
        let mut last = 0.0;
        for _ in 0..44100 {
            last = bq.tick(0.5);
        }
        assert!(last.abs() < 1e-6);

        let mut bq = Biquad::highpass(62.5, 44100.0, 2.0);
        let mut peak = 0.0f64;
        for i in 0..44100 {
            let x = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44100.0).sin();
            let y = bq.tick(x);
            if i > 4410 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - 1.0).abs() < 0.01);
    }
}
