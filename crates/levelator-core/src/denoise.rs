//! Background-noise reduction (an addition; the original Levelator had none).
//!
//! A conservative STFT Wiener filter aimed at steady background noise: hiss,
//! static, fan or air-conditioning hum.
//!
//! 1. The noise spectrum is learned from the quietest 10% of the recording
//!    (the pauses between speech). If the file has no clear pauses, the
//!    estimate would include speech, so the filter does nothing.
//! 2. Every frame is filtered with a decision-directed Wiener gain
//!    (Ephraim–Malah a-priori SNR). It changes smoothly over time and is
//!    smoothed across neighbouring frequencies, which avoids the
//!    warbling "musical noise" of plain spectral subtraction.
//! 3. The gain never drops below a floor (-12 dB for [`Strength::Mild`]),
//!    so noise is turned down rather than gated out and speech keeps its
//!    natural timbre.
//!
//! Stereo files get one gain per frame and bin, computed from both channels,
//! so the stereo image doesn't shift. Processing is in place with a
//! frame-sized working buffer, so long files need no extra copy.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::stages::{Progress, Signal};
use crate::Error;

/// How hard to push the noise down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strength {
    /// Up to 12 dB of noise reduction; transparent on speech.
    #[default]
    Mild,
    /// Up to 18 dB.
    Medium,
    /// Up to 24 dB; may start to colour quiet speech.
    Strong,
}

impl Strength {
    /// (gain floor in dB, noise over-estimation factor)
    fn params(self) -> (f64, f64) {
        match self {
            Strength::Mild => (-12.0, 1.0),
            Strength::Medium => (-18.0, 1.3),
            Strength::Strong => (-24.0, 1.8),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Strength::Mild => "mild",
            Strength::Medium => "medium",
            Strength::Strong => "strong",
        }
    }

    pub fn from_name(s: &str) -> Option<Strength> {
        match s.to_ascii_lowercase().as_str() {
            "mild" | "low" => Some(Strength::Mild),
            "medium" => Some(Strength::Medium),
            "strong" | "high" => Some(Strength::Strong),
            _ => None,
        }
    }
}

/// Smoothing factor of the decision-directed a-priori SNR estimate.
const DD_ALPHA: f64 = 0.98;
/// Fraction of (non-silent) frames used to learn the noise spectrum.
const NOISE_QUANTILE: f64 = 0.10;
/// The quietest frames must be at least this much (dB) below the 75th
/// percentile frame, otherwise there are no real pauses and the filter is
/// skipped.
const MIN_PAUSE_CONTRAST_DB: f64 = 10.0;
/// Mean pause energy below this (-100 dBFS) means the pauses are silent.
const SILENCE_ENERGY: f64 = 1e-10;

struct Stft {
    n: usize,
    hop: usize,
    window: Vec<f64>,
    fwd: Arc<dyn RealToComplex<f64>>,
    inv: Arc<dyn ComplexToReal<f64>>,
    /// Overlap-add normalisation for analysis * synthesis windows.
    ola_norm: f64,
}

impl Stft {
    fn new(sample_rate: i32) -> Self {
        // ~40 ms frames (2048 at 44.1/48 kHz), 75% overlap.
        let n = ((sample_rate.max(8000) as f64 * 0.04) as usize).next_power_of_two();
        let hop = n / 4;
        // sqrt-Hann (periodic) for analysis and synthesis.
        let window: Vec<f64> = (0..n)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).sqrt())
            .collect();
        let ola_norm = (0..n / hop).map(|k| window[k * hop].powi(2)).sum::<f64>();
        let mut planner = RealFftPlanner::<f64>::new();
        Stft {
            n,
            hop,
            fwd: planner.plan_fft_forward(n),
            inv: planner.plan_fft_inverse(n),
            window,
            ola_norm,
        }
    }

    fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Start positions (may be negative) of every frame covering `0..len`.
    fn frame_starts(&self, len: usize) -> impl Iterator<Item = isize> + use<> {
        let (n, hop) = (self.n as isize, self.hop as isize);
        (-(n - hop)..len as isize).step_by(self.hop)
    }

    /// Windowed frame of channel `c` starting at `start` (zero outside the signal).
    fn load(&self, sig: &Signal, c: usize, start: isize, out: &mut [f64]) {
        let ch = sig.channels;
        let frames = sig.frames() as isize;
        for (i, (o, w)) in out.iter_mut().zip(&self.window).enumerate() {
            let t = start + i as isize;
            *o = if t >= 0 && t < frames {
                sig.data[t as usize * ch + c] * w
            } else {
                0.0
            };
        }
    }
}

/// Reduce steady background noise in place. Returns `false` (and leaves the
/// signal untouched) when no noise profile could be learned.
pub fn run(sig: &mut Signal, strength: Strength, progress: &mut Progress) -> Result<bool, Error> {
    let ch = sig.channels;
    let frames = sig.frames();
    let stft = Stft::new(sig.sample_rate);
    let (n, hop, nb) = (stft.n, stft.hop, stft.bins());
    if frames < n * 2 {
        return Ok(false);
    }

    let mut time = vec![0.0f64; n];
    let mut spec = vec![Complex::new(0.0, 0.0); nb];
    let mut scratch = stft.fwd.make_scratch_vec();

    // Pass 1: energy of every frame (mono mix, no FFT needed).
    let starts: Vec<isize> = stft.frame_starts(frames).collect();
    let energy: Vec<f64> = starts
        .iter()
        .map(|&s| {
            let lo = s.max(0) as usize;
            let hi = ((s + n as isize).max(0) as usize).min(frames);
            sig.data[lo * ch..hi * ch]
                .iter()
                .map(|x| x * x)
                .sum::<f64>()
                / n as f64
        })
        .collect();
    // Statistics only over frames fully inside the signal.
    let inside: Vec<usize> = (0..starts.len())
        .filter(|&i| starts[i] >= 0 && starts[i] as usize + n <= frames)
        .collect();
    if inside.len() < 20 {
        return Ok(false);
    }
    let mut sorted: Vec<f64> = inside.iter().map(|&i| energy[i]).collect();
    sorted.sort_by(f64::total_cmp);
    let loud = sorted[sorted.len() * 3 / 4];
    let threshold = sorted[((sorted.len() as f64 * NOISE_QUANTILE) as usize).max(1) - 1];
    let quiet: Vec<usize> = inside
        .iter()
        .copied()
        .filter(|&i| energy[i] <= threshold)
        .collect();
    let quiet_mean = quiet.iter().map(|&i| energy[i]).sum::<f64>() / quiet.len() as f64;
    // Pauses already (near) digitally silent: nothing to remove.
    if quiet_mean < SILENCE_ENERGY {
        return Ok(false);
    }
    // No real pauses: the quietest frames are speech, not noise.
    if 10.0 * (loud / quiet_mean).log10() < MIN_PAUSE_CONTRAST_DB {
        return Ok(false);
    }
    progress.update(0.0, 10.0, 0.3)?;

    // Pass 2: average power spectrum of the quiet frames = noise profile.
    let mut noise = vec![0.0f64; nb];
    for &i in &quiet {
        for c in 0..ch {
            stft.load(sig, c, starts[i], &mut time);
            stft.fwd
                .process_with_scratch(&mut time, &mut spec, &mut scratch)
                .ok();
            for (acc, z) in noise.iter_mut().zip(&spec) {
                *acc += z.norm_sqr();
            }
        }
    }
    let (floor_db, over) = strength.params();
    let g_min = 10f64.powf(floor_db / 20.0);
    let scale = over / (quiet.len() * ch) as f64;
    noise.iter_mut().for_each(|v| *v = (*v * scale).max(1e-30));
    progress.update(0.0, 10.0, 0.5)?;

    // Pass 3: filter and overlap-add in place. Output for samples before the
    // current frame start is final, and later frames only read samples at or
    // after their own start, so finished samples can be written straight
    // back into the signal.
    let mut acc = vec![vec![0.0f64; n]; ch];
    let mut chan_spec = vec![vec![Complex::new(0.0, 0.0); nb]; ch];
    let mut inv_scratch = stft.inv.make_scratch_vec();
    let mut prev_gain = vec![1.0f64; nb];
    let mut prev_post = vec![1.0f64; nb];
    let mut gain = vec![0.0f64; nb];
    let mut smooth = vec![0.0f64; nb];
    let norm = 1.0 / (stft.ola_norm * n as f64); // inverse FFT is unnormalised
    let total = starts.len();
    let mut power = vec![0.0f64; nb];
    for (fi, &start) in starts.iter().enumerate() {
        power.fill(0.0);
        for (c, cs) in chan_spec.iter_mut().enumerate() {
            stft.load(sig, c, start, &mut time);
            stft.fwd
                .process_with_scratch(&mut time, cs, &mut scratch)
                .ok();
            for (p, z) in power.iter_mut().zip(cs.iter()) {
                *p += z.norm_sqr() / ch as f64;
            }
        }
        for k in 0..nb {
            let post = power[k] / noise[k];
            let prio = DD_ALPHA * prev_gain[k] * prev_gain[k] * prev_post[k]
                + (1.0 - DD_ALPHA) * (post - 1.0).max(0.0);
            gain[k] = (prio / (1.0 + prio)).max(g_min);
            prev_post[k] = post;
        }
        for k in 0..nb {
            let l = gain[k.saturating_sub(1)];
            let r = gain[(k + 1).min(nb - 1)];
            smooth[k] = 0.25 * l + 0.5 * gain[k] + 0.25 * r;
        }
        prev_gain.copy_from_slice(&gain);

        for c in 0..ch {
            for (z, g) in chan_spec[c].iter_mut().zip(&smooth) {
                *z *= *g;
            }
            // The DC and Nyquist bins of a real signal must be real.
            chan_spec[c][0].im = 0.0;
            chan_spec[c][nb - 1].im = 0.0;
            stft.inv
                .process_with_scratch(&mut chan_spec[c], &mut time, &mut inv_scratch)
                .ok();
            for i in 0..n {
                acc[c][i] += time[i] * stft.window[i] * norm;
            }
            // Samples start..start+hop are now complete.
            for (i, &y) in acc[c][..hop].iter().enumerate() {
                let t = start + i as isize;
                if t >= 0 && (t as usize) < frames {
                    sig.data[t as usize * ch + c] = y;
                }
            }
            acc[c].copy_within(hop.., 0);
            acc[c][n - hop..].fill(0.0);
        }
        if fi % 256 == 0 {
            progress.update(0.0, 10.0, 0.5 + 0.5 * fi as f64 / total as f64)?;
        }
    }
    // Flush the tail still held in the accumulator.
    let last = *starts.last().unwrap() + hop as isize;
    for (c, a) in acc.iter().enumerate() {
        for (i, &y) in a[..n - hop].iter().enumerate() {
            let t = last + i as isize;
            if t >= 0 && (t as usize) < frames {
                sig.data[t as usize * ch + c] = y;
            }
        }
    }
    progress.update(0.0, 10.0, 1.0)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioFormat, Container, Encoding};
    use std::sync::atomic::AtomicBool;

    const SR: i32 = 22050;

    fn lcg(state: &mut u64) -> f64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }

    /// Alternating 1 s of voiced "speech" and 1 s of pause, plus the noise.
    fn fixture(noise_amp: f64) -> (Vec<f64>, Vec<f64>, Vec<bool>) {
        let n = SR as usize * 12;
        let mut rng = 3u64;
        let mut clean = vec![0.0; n];
        let mut voiced = vec![false; n];
        let mut phase = 0.0f64;
        for i in 0..n {
            let t = i as f64 / SR as f64;
            if (t as usize) % 2 == 1 {
                let env = (std::f64::consts::PI * 4.0 * t).sin().powi(2);
                phase += std::f64::consts::TAU * (130.0 + 20.0 * (t * 3.0).sin()) / SR as f64;
                clean[i] = 0.1
                    * env
                    * (1..12)
                        .map(|k| (k as f64 * phase).sin() / k as f64)
                        .sum::<f64>();
                voiced[i] = env > 0.3;
            }
        }
        // White noise ~ uniform, scaled to the requested RMS.
        let noisy = clean
            .iter()
            .map(|c| c + noise_amp * 3f64.sqrt() * lcg(&mut rng))
            .collect();
        (clean, noisy, voiced)
    }

    fn rms_db(it: impl Iterator<Item = f64>) -> f64 {
        let (s, n) = it.fold((0.0, 0usize), |(s, n), x| (s + x * x, n + 1));
        10.0 * (s / n as f64).log10()
    }

    fn run_on(data: Vec<f64>, strength: Strength) -> (Vec<f64>, bool) {
        let format = AudioFormat {
            container: Container::Wav,
            encoding: Encoding::Float(64),
            sample_rate: SR as u32,
            channels: 1,
        };
        let mut sig = Signal {
            data,
            channels: 1,
            sample_rate: SR,
            format,
        };
        let cancel = AtomicBool::new(false);
        let mut cb = |_| {};
        let mut p = Progress::new(&mut cb, &cancel);
        let applied = run(&mut sig, strength, &mut p).unwrap();
        (sig.data, applied)
    }

    #[test]
    fn mild_reduces_hiss_and_keeps_speech() {
        let noise = 10f64.powf(-50.0 / 20.0);
        let (clean, noisy, voiced) = fixture(noise);
        let (out, applied) = run_on(noisy.clone(), Strength::Mild);
        assert!(applied);
        // Pauses: whole seconds 2, 4, 6, ... (skip the first and last second).
        let pause = |v: &[f64]| {
            rms_db(
                (2..10)
                    .step_by(2)
                    .flat_map(|s| v[s * SR as usize + 2000..(s + 1) * SR as usize - 2000].to_vec()),
            )
        };
        let before = pause(&noisy);
        let after = pause(&out);
        // Speech: error relative to the clean signal where voiced.
        let err = |v: &[f64]| {
            rms_db(
                v.iter()
                    .zip(&clean)
                    .zip(&voiced)
                    .filter(|(_, &m)| m)
                    .map(|((a, b), _)| a - b),
            )
        };
        let speech = |v: &[f64]| rms_db(v.iter().zip(&voiced).filter(|(_, &m)| m).map(|(a, _)| *a));
        eprintln!(
            "pause noise {before:.1} -> {after:.1} dB; speech error {:.1} -> {:.1} dB; speech level {:.2} -> {:.2} dB",
            err(&noisy), err(&out), speech(&clean), speech(&out)
        );
        assert!(
            before - after > 9.0,
            "noise only reduced {:.1} dB",
            before - after
        );
        assert!(
            before - after < 13.0,
            "mild must not gate harder than its floor"
        );
        assert!(
            (speech(&out) - speech(&clean)).abs() < 0.5,
            "speech level changed"
        );
    }

    #[test]
    fn leaves_files_without_pauses_alone() {
        // Continuous tone + noise: no quiet frames to learn the noise from.
        let data: Vec<f64> = (0..SR as usize * 5)
            .map(|i| 0.2 * (i as f64 * 0.05).sin())
            .collect();
        let (out, applied) = run_on(data.clone(), Strength::Mild);
        assert!(!applied);
        assert_eq!(out, data);
    }

    #[test]
    fn unity_when_noise_free() {
        // Pure speech + digital silence in the pauses reconstructs exactly-ish.
        let (clean, _, _) = fixture(0.0);
        let (out, _) = run_on(clean.clone(), Strength::Mild);
        let e = rms_db(out.iter().zip(&clean).map(|(a, b)| a - b));
        assert!(e < -80.0, "reconstruction error {e:.1} dB");
    }
}
