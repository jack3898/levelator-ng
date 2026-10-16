//! Step 10: `normalize` (0x40522, NormalizeOperation).
//!
//! Finds, by secant iteration, the broadband gain G that brings the RMS of
//! the active (above noise floor) bins to the target level (-18 dB). Bins
//! whose peak would exceed -1 dBFS are limited, with 2 s attack/release
//! ramps around them, so the result is both loud and peak-safe.

use super::{Progress, Signal};
use crate::dsp::from_db;
use crate::Error;

#[derive(Clone, Debug)]
pub struct NormalizeArgs {
    pub target_level_db: f64,
    pub fit_tolerance_db: f64,
    pub attack_ms: i32,
    pub release_ms: i32,
    pub noise_floor_db: f64,
    pub peak_output_db: f64,
    pub bin_ms: i32,
    /// Output bit depth (8/16/24/32) or `None` to keep the input's.
    pub bit_depth: Option<u16>,
}

impl Default for NormalizeArgs {
    fn default() -> Self {
        NormalizeArgs {
            target_level_db: -18.0,
            fit_tolerance_db: 0.01,
            attack_ms: 2000,
            release_ms: 2000,
            noise_floor_db: -44.0,
            peak_output_db: -1.0,
            bin_ms: 50,
            bit_depth: None,
        }
    }
}

/// The original iterated without limit; a level that stops responding to the
/// gain would hang it. Stop after this many steps and keep the last curve.
const MAX_ITERATIONS: usize = 1000;

#[derive(Clone, Copy)]
struct Bin {
    gain: f64,
    peak: f64,
    rms: f64,
}

struct GainCalc<'a> {
    bins: &'a mut [Bin],
    gains: Vec<f64>,
    noise: f64,
    peak_lin: f64,
    attack_bins: i64,
    release_bins: i64,
}

impl GainCalc<'_> {
    /// 0x400ce + 0x3ea3c: per-bin peak-limited gain and ramped boundary curve.
    fn curve(&mut self, g: f64) {
        for b in self.bins.iter_mut() {
            b.gain = if self.noise < b.rms {
                let q = self.peak_lin / b.peak;
                if q <= g {
                    q
                } else {
                    g
                }
            } else {
                g
            };
        }
        let n = self.bins.len() as i64;
        self.gains.iter_mut().for_each(|x| *x = g);
        let minsd = |v: f64, x: f64| if v < x { v } else { x };
        for i in 0..n {
            let b = self.bins[i as usize].gain;
            if b < g {
                let step = (g - b) / (self.release_bins + 1) as f64;
                let end = (i + 2 + self.release_bins).min(n + 1);
                let mut v = b;
                for j in (i + 1)..end {
                    self.gains[j as usize] = minsd(v, self.gains[j as usize]);
                    v = step + v;
                }
                let step = (g - b) / (self.attack_bins + 1) as f64;
                let start = (i - self.attack_bins - 1).max(-1);
                let mut v = b;
                let mut j = i;
                while j > start {
                    self.gains[j as usize] = minsd(v, self.gains[j as usize]);
                    j -= 1;
                    v = step + v;
                }
            }
        }
    }

    /// 0x40260: RMS level (dB) of the active bins after the gain curve.
    fn level(&self) -> f64 {
        let mut sum = 0.0f64;
        let mut cnt = 0u32;
        for i in 0..self.bins.len().saturating_sub(1) {
            let r = self.bins[i].rms;
            if self.noise < r {
                cnt += 1;
                let v = r * ((self.gains[i] + self.gains[i + 1]) * 0.5);
                sum = sum + v * v;
            }
        }
        if sum == 0.0 || cnt == 0 {
            -200.0
        } else {
            10.0 * (sum / cnt as f64).log10()
        }
    }
}

fn secant(x0: f64, x1: f64, y0: f64, y1: f64, t: f64) -> f64 {
    if y0 != y1 {
        x0 + ((t - y0) * (x1 - x0)) / (y1 - y0)
    } else {
        x0
    }
}

pub fn run(
    sig: Signal,
    args: &NormalizeArgs,
    progress: &mut Progress,
    name: &str,
) -> Result<Signal, Error> {
    let mut peak_db = args.peak_output_db;
    if peak_db > 0.0 {
        peak_db = -peak_db;
        eprintln!("Warning: Peak must be negative.  Value has been changed to {peak_db:.2}.");
    }
    let mut noise_db = args.noise_floor_db;
    if noise_db > 0.0 {
        noise_db = -noise_db;
        eprintln!("Warning: Specified noise floor was positive.  Value has been changed to {noise_db:.2}.");
    }
    let peak_lin = from_db(peak_db);
    let noise = from_db(noise_db);
    let target_lin = from_db(args.target_level_db);

    let ch = sig.channels;
    if ch >= 3 {
        return Err(Error::Format(format!(
            "{name}: cannot process more than 2 channels;  has {ch}"
        )));
    }
    let bin_frames = (sig.sample_rate * args.bin_ms) / 1000;
    if bin_frames <= 0 {
        return Err(Error::Format(format!("{name}: sample rate too low")));
    }
    let bin_frames = bin_frames as usize;
    let bin_samples = bin_frames * ch;
    let nbins = sig.frames().div_ceil(bin_frames);

    // Analysis (0x40ade).
    let mut bins = Vec::with_capacity(nbins);
    let mut tot_sq = 0.0f64;
    let mut tot_n: i64 = 0;
    for b in 0..nbins {
        let s = &sig.data[b * bin_samples..((b + 1) * bin_samples).min(sig.data.len())];
        if s.is_empty() {
            return Err(Error::Format(format!("{name}: Expected more samples")));
        }
        let k = s.len();
        let (mut pk, mut s0, mut s1) = (0.0f64, 0.0f64, 0.0f64);
        let mut j = 0;
        while j < k {
            let a = s[j].abs();
            if a > pk {
                pk = a;
            }
            s0 = a * a + s0;
            if ch == 2 {
                j += 1;
                let a = s[j].abs();
                if !(a <= pk) {
                    pk = a;
                }
                s1 = a * a + s1;
            }
            j += 1;
        }
        let m = (k / ch) as f64;
        let rms = if ch == 1 {
            (s0 / m).sqrt()
        } else {
            let r0 = (s0 / m).sqrt();
            let r1 = (s1 / m).sqrt();
            if r1 <= r0 {
                r0
            } else {
                r1
            }
        };
        bins.push(Bin {
            gain: 1.0,
            peak: pk,
            rms,
        });
        if noise < rms {
            tot_sq = tot_sq + if ch == 1 { s0 } else { s0 + s1 };
            tot_n += k as i64;
        }
        progress.update(91.0, 95.0, Progress::frac(b, nbins))?;
    }
    let overall = if tot_n == 0 {
        0.0
    } else {
        (tot_sq / tot_n as f64).sqrt()
    };
    if overall == 0.0 {
        return Err(Error::Silent(name.to_owned()));
    }

    // Secant search for the broadband gain (0x403a2).
    let target_db = 20.0 * target_lin.log10();
    let mut calc = GainCalc {
        bins: &mut bins,
        gains: vec![0.0; nbins + 1],
        noise,
        peak_lin,
        attack_bins: (args.attack_ms / args.bin_ms) as i64,
        release_bins: (args.release_ms / args.bin_ms) as i64,
    };
    let mut g_prev = 1.0;
    calc.curve(1.0);
    let mut l_prev = calc.level();
    let mut g = target_lin / overall;
    for _ in 0..MAX_ITERATIONS {
        calc.curve(g);
        let l = calc.level();
        let g_next = secant(g_prev, g, l_prev, l, target_db);
        if (l - target_db).abs() <= args.fit_tolerance_db {
            break;
        }
        g_prev = g;
        g = g_next;
        l_prev = l;
    }
    let gains = calc.gains;

    // Apply and write (0x411dc).
    let mut out = sig;
    if let Some(bits) = args.bit_depth {
        out.format.encoding = crate::audio::Encoding::Pcm(bits);
    }
    for i in 0..nbins {
        let start = i * bin_samples;
        let end = ((i + 1) * bin_samples).min(out.data.len());
        let mut g = gains[i];
        let d = (gains[i + 1] - gains[i]) / bin_frames as f64;
        for frame in out.data[start..end].chunks_mut(ch) {
            for x in frame {
                *x = g * *x;
            }
            g = g + d;
        }
        progress.update(95.0, 100.0, Progress::frac(i, nbins))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioFormat, Container, Encoding};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn reaches_target_level() {
        let sr = 8000;
        let data: Vec<f64> = (0..sr * 10)
            .map(|i| 0.05 * (i as f64 * 0.1).sin())
            .collect();
        let format = AudioFormat {
            container: Container::Wav,
            encoding: Encoding::Float(64),
            sample_rate: sr as u32,
            channels: 1,
        };
        let sig = Signal {
            data,
            channels: 1,
            sample_rate: sr,
            format,
        };
        let cancel = AtomicBool::new(false);
        let mut cb = |_| {};
        let mut p = Progress::new(&mut cb, &cancel);
        let out = run(sig, &NormalizeArgs::default(), &mut p, "t").unwrap();
        let mid = &out.data[sr as usize * 4..sr as usize * 6];
        let rms = (mid.iter().map(|x| x * x).sum::<f64>() / mid.len() as f64).sqrt();
        assert!(
            (20.0 * rms.log10() + 18.0).abs() < 0.05,
            "rms {}",
            20.0 * rms.log10()
        );
    }
}
