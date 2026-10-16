//! Step 7: `calculate_gains` (0x3646c → 0x350b6).
//!
//! Measures the gated RMS level of every segment between cue points, takes
//! the plain mean (in dB) of those levels as the target, and gives every
//! segment the gain that moves it to the target, soft-limited to ±24 dB with
//! `24 * tanh(dev / 24)`. The result is one linear gain per 50 ms bin.

use super::{Progress, Signal};
use crate::dsp::{sat_u32, via_fixed, via_g6};
use crate::Error;

#[derive(Clone, Debug)]
pub struct GainArgs {
    /// Samples count towards a segment's RMS only while the recent peak exceeds this (linear).
    pub noise_threshold: f64,
    pub bin_ms: u32,
    /// Soft limit for the gain change in dB (`None` = unlimited).
    pub gain_limit_db: Option<f64>,
    /// Length of the peak-hold window used by the gate, in seconds.
    pub buffer_length_s: f64,
}

impl Default for GainArgs {
    fn default() -> Self {
        GainArgs {
            noise_threshold: 0.006,
            bin_ms: 50,
            gain_limit_db: Some(24.0),
            buffer_length_s: 0.005,
        }
    }
}

/// Gated RMS (CalcRMS, 0x343d2): a sample counts while the maximum |x| over
/// the trailing `ring_len` interleaved samples is above `thr`.
fn gated_rms(data: &[f64], ring_len: usize, thr: f64) -> f64 {
    let mut ring = vec![0.0f64; ring_len.max(1)];
    let mut wp = 0usize;
    let mut peak = 0.0f64;
    let mut sum = 0.0f64;
    let mut cnt: u64 = 0;
    for &x in data {
        let a = x.abs();
        let rescan = if a > peak {
            peak = a;
            false
        } else {
            !(peak > ring[wp])
        };
        ring[wp] = a;
        wp += 1;
        if rescan {
            peak = 0.0;
            for &r in &ring {
                peak = if r > peak { r } else { peak };
            }
        }
        if peak > thr {
            cnt += 1;
            sum = a * a + sum;
        }
        if wp > ring.len() - 1 {
            wp = 0;
        }
    }
    if cnt == 0 {
        0.0
    } else {
        (sum / cnt as f64).sqrt()
    }
}

/// Returns `(gains, bin_frames)`: one linear gain per bin, as `_08.txt` held them.
pub fn run(
    sig: &Signal,
    cues: &[u64],
    args: &GainArgs,
    progress: &mut Progress,
) -> Result<(Vec<f64>, u32), Error> {
    let frames = sig.frames() as u64;
    let ch = sig.channels as u64;
    let sr = sig.sample_rate as u32;
    let ring_len =
        (ch as u32).wrapping_mul(sat_u32(sr as f64 * args.buffer_length_s + 1.0)) as usize;
    let bin = sr.wrapping_mul(args.bin_ms) / 1000;
    if bin == 0 {
        return Err(Error::Format("sample rate too low".into()));
    }
    let round_cue = |c: u64| bin as u64 * sat_u32(c as f64 / bin as f64 + 0.5) as u64;

    // Pass 1: per-segment levels (the `_07.txt` RMS file). The sound file is
    // read sequentially and never seeked.
    let mut levels = Vec::with_capacity(cues.len() + 1);
    let mut pos: usize = 0;
    let mut prev: u64 = 0;
    let mut sum_db = 0.0f64;
    let mut count: u32 = 0;
    for end in cues
        .iter()
        .map(|&c| round_cue(c))
        .chain(std::iter::once(frames))
    {
        let len = (ch as i64).wrapping_mul(end.wrapping_sub(prev) as i64);
        let take = if len <= 0 {
            0
        } else {
            (len as u64).min((sig.data.len() - pos) as u64) as usize
        };
        let rms = gated_rms(&sig.data[pos..pos + take], ring_len, args.noise_threshold);
        pos += take;
        let v = if rms > 0.0 {
            let v = 20.0 * rms.log10();
            count += 1;
            sum_db = sum_db + v;
            v
        } else {
            -200.0
        };
        levels.push(via_fixed(v, 2));
        prev = end;
    }
    let mean = sum_db / count as f64;

    // Pass 2: gains file. Iterates `count` times over the level lines, exactly
    // like the original (which misaligns if a segment measured -200).
    let mut gains = Vec::new();
    prev = 0;
    for i in 0..count as usize {
        let mut x = levels[i] - mean;
        if let Some(limit) = args.gain_limit_db {
            let l = limit.abs();
            x = (x / l).tanh() * l;
        }
        let g = via_g6(1.0 / 10f64.powf(x / 20.0));
        let end = if i == count as usize - 1 {
            frames + 1
        } else {
            round_cue(cues[i])
        };
        let n = (end.wrapping_sub(prev).wrapping_add(bin as u64 - 1) / bin as u64) as u32;
        gains.extend(std::iter::repeat_n(g, n as usize));
        if count > 1 {
            progress.update(69.0, 82.0, i as f64 / count as f64)?;
        }
        prev = end;
    }
    progress.update(69.0, 82.0, 1.0)?;
    Ok((gains, bin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_ignores_silence() {
        let mut x = vec![0.0; 1000];
        x.extend(vec![0.5; 1000]);
        x.extend(vec![0.0; 1000]);
        let r = gated_rms(&x, 10, 0.006);
        // 1000 loud samples plus the 9 trailing zeros still inside the hold window.
        assert!((r - (0.25f64 * 1000.0 / 1009.0).sqrt()).abs() < 1e-12);
    }
}
