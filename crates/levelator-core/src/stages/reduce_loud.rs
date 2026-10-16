//! Steps 3 and 4: `reduce_loud_peak` and `reduce_loud`.
//!
//! Both split the file into fixed bins, measure the mean RMS of the bins that
//! are above a noise floor, and pull down every bin whose loudness exceeds
//! `mean * 10^(level_diff/20)`. Gains are defined at bin boundaries, spread
//! with attack/release ramps, linearly interpolated per frame, and finally the
//! whole curve is scaled so the loudest post-gain bin peak lands at 1.0.
//!
//! The two variants differ only in what "loudness" means: the peak variant uses
//! the bin's sample peak, the RMS variant uses the louder channel's RMS
//! (original functions 0x4a322 and 0x466be).

use super::{Progress, Signal};
use crate::dsp::from_db;
use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    /// `reduce_loud_peak`: compare the bin peak (|x| max over all channels).
    Peak,
    /// `reduce_loud`: compare max(RMS_left, RMS_right).
    Rms,
}

#[derive(Clone, Debug)]
pub struct ReduceLoudArgs {
    pub measure: Measure,
    pub noise_floor_db: f64,
    pub level_diff_db: f64,
    pub attack_ms: i32,
    pub release_ms: i32,
    pub bin_ms: i32,
}

impl ReduceLoudArgs {
    /// Values the 2.1.2 pipeline passes to step 3.
    pub fn peak_defaults() -> Self {
        ReduceLoudArgs {
            measure: Measure::Peak,
            noise_floor_db: -44.0,
            level_diff_db: 15.0,
            attack_ms: 0,
            release_ms: 0,
            bin_ms: 50,
        }
    }

    /// Values the 2.1.2 pipeline passes to step 4.
    pub fn rms_defaults() -> Self {
        ReduceLoudArgs {
            measure: Measure::Rms,
            noise_floor_db: -44.0,
            level_diff_db: 3.0,
            attack_ms: 0,
            release_ms: 150,
            bin_ms: 50,
        }
    }
}

#[derive(Clone, Copy)]
struct Bin {
    gain: f64,
    peak: f64,
    rms: f64,
}

pub fn run(
    sig: Signal,
    args: &ReduceLoudArgs,
    progress: &mut Progress,
    name: &str,
) -> Result<Signal, Error> {
    let mut noise_floor_db = args.noise_floor_db;
    if noise_floor_db > 0.0 {
        noise_floor_db = -noise_floor_db;
        eprintln!("Warning: Specified noise floor was positive.  Value has been changed to {noise_floor_db:.2}.");
    }
    let level_lin = from_db(args.level_diff_db);
    let noise_lin = from_db(noise_floor_db);

    let ch = sig.channels;
    if ch > 2 {
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
    let frames = sig.frames();
    let n_bins = frames.div_ceil(bin_frames);
    let (p_lo, p_hi, w_lo, w_hi) = match args.measure {
        Measure::Peak => (20.0, 24.0, 24.0, 27.0),
        Measure::Rms => (27.0, 30.0, 30.0, 34.0),
    };

    // Pass 1: measure every bin and the mean RMS of the non-quiet ones.
    let mut bins = Vec::with_capacity(n_bins);
    let mut total = 0.0f64;
    let mut count = 0u64;
    for b in 0..n_bins {
        let s = &sig.data[b * bin_samples..((b + 1) * bin_samples).min(sig.data.len())];
        if s.is_empty() {
            return Err(Error::Format(format!("{name}: Expected more samples")));
        }
        let n = s.len();
        let mut peak = 0.0f64;
        let bin = match args.measure {
            Measure::Peak => {
                let mut ss = 0.0f64;
                for &x in s {
                    let a = x.abs();
                    if a > peak {
                        peak = a;
                    }
                    ss = a * a + ss;
                }
                let rms = (ss / n as f64).sqrt();
                if !(noise_lin >= rms) {
                    total = total + ss;
                    count += n as u64;
                }
                Bin {
                    gain: 1.0,
                    peak,
                    rms,
                }
            }
            Measure::Rms => {
                let (mut s0, mut s1) = (0.0f64, 0.0f64);
                let mut k = 0;
                while k < n {
                    let a = s[k].abs();
                    if a > peak {
                        peak = a;
                    }
                    s0 = a * a + s0;
                    if ch == 2 {
                        k += 1;
                        let a = s[k].abs();
                        if a > peak {
                            peak = a;
                        }
                        s1 = a * a + s1;
                    }
                    k += 1;
                }
                let fr = (n / ch) as f64;
                let (rms, sum) = if ch == 1 {
                    ((s0 / fr).sqrt(), s0)
                } else {
                    let l = (s0 / fr).sqrt();
                    let r = (s1 / fr).sqrt();
                    (if r > l { r } else { l }, s0 + s1)
                };
                if !(noise_lin >= rms) {
                    total = total + sum;
                    count += n as u64;
                }
                Bin {
                    gain: 1.0,
                    peak,
                    rms,
                }
            }
        };
        bins.push(bin);
        progress.update(p_lo, p_hi, Progress::frac(b, n_bins))?;
    }
    let mean = if count == 0 {
        0.0
    } else {
        (total / count as f64).sqrt()
    };
    if mean == 0.0 {
        return Err(Error::Silent(name.to_owned()));
    }

    // Per-bin gains and the largest post-gain peak.
    let threshold = mean * level_lin;
    let mut max_peak = 0.0f64;
    for b in bins.iter_mut() {
        let level = match args.measure {
            Measure::Peak => b.peak,
            Measure::Rms => b.rms,
        };
        if level > threshold {
            b.gain = threshold / level;
            let p = b.peak * b.gain;
            if p > max_peak {
                max_peak = p;
            }
        } else if b.peak > max_peak {
            max_peak = b.peak;
        }
    }

    let att = (args.attack_ms / args.bin_ms) as i64;
    let rel = (args.release_ms / args.bin_ms) as i64;
    let mut gains = gain_curve(&bins, att, rel);
    let f = 1.0 / max_peak;
    for g in gains.iter_mut() {
        *g = f * *g;
    }

    // Pass 2: apply the interpolated boundary gains.
    let mut out = sig;
    for b in 0..n_bins {
        let start = b * bin_samples;
        let end = ((b + 1) * bin_samples).min(out.data.len());
        let mut g = gains[b];
        let step = (gains[b + 1] - gains[b]) / bin_frames as f64;
        for frame in out.data[start..end].chunks_mut(ch) {
            for x in frame {
                *x = g * *x;
            }
            g = g + step;
        }
        progress.update(w_lo, w_hi, Progress::frac(b, n_bins))?;
    }
    out.requantize();
    Ok(out)
}

/// Boundary gains (n_bins + 1 entries) with linear attack/release ramps
/// around every attenuated bin (0x494f0 / 0x4561a).
fn gain_curve(bins: &[Bin], att: i64, rel: i64) -> Vec<f64> {
    let n = bins.len() as i64;
    let mut gains = vec![1.0f64; bins.len() + 1];
    let minsd = |cur: f64, x: f64| if cur < x { cur } else { x };
    for i in 0..n {
        let g = bins[i as usize].gain;
        if g < 1.0 {
            let step_r = (1.0 - g) / (rel + 1) as f64;
            let end = (rel + i + 2).min(n + 1);
            let mut cur = g;
            for j in (i + 1)..end {
                gains[j as usize] = minsd(cur, gains[j as usize]);
                cur = step_r + cur;
            }
            let step_a = (1.0 - g) / (att + 1) as f64;
            let lower = (i - att - 1).max(-1);
            let mut cur = g;
            let mut j = i;
            while j > lower {
                gains[j as usize] = minsd(cur, gains[j as usize]);
                cur = step_a + cur;
                j -= 1;
            }
        }
    }
    gains
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_ramp_shape() {
        let mut bins = vec![
            Bin {
                gain: 1.0,
                peak: 0.0,
                rms: 0.0
            };
            10
        ];
        bins[2].gain = 0.5;
        let g = gain_curve(&bins, 0, 3);
        assert_eq!(&g[..8], &[1.0, 1.0, 0.5, 0.5, 0.625, 0.75, 0.875, 1.0]);
    }
}
