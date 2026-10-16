//! Step 6: `wrap_segment` (0x55774) and `segment` (0x4f920 / 0x4da82).
//!
//! Drops quiet bins, then splits the remaining dB sequence into piecewise
//! constant segments by dynamic programming (minimum total sum of squared
//! deviations), adding breakpoints until the RMS residual is within the
//! tolerance. The breakpoints become cue positions (in frames).

use super::Progress;
use crate::dsp::{from_db, via_g6};
use crate::Error;

#[derive(Clone, Debug)]
pub struct SegmentArgs {
    /// Bins quieter than (mean level + quiet_delta) dB are ignored.
    pub quiet_delta_db: f64,
    /// Bin length in seconds used to turn bin indices into frame positions.
    pub bin_s: f64,
    /// Maximum RMS residual (dB) of the fit.
    pub tolerance: f64,
    pub min_segment_length: u16,
    pub max_breakpoints: u16,
    pub chunk_size: u16,
}

impl Default for SegmentArgs {
    fn default() -> Self {
        SegmentArgs { quiet_delta_db: -12.0, bin_s: 0.05, tolerance: 3.5, min_segment_length: 20, max_breakpoints: 30, chunk_size: 2400 }
    }
}

/// Cue positions in frames (what `_06.clt` held).
pub fn wrap_segment(rms_db: &[f64], sample_rate: i32, args: &SegmentArgs, progress: &mut Progress) -> Result<Vec<u64>, Error> {
    if args.quiet_delta_db > 0.0 {
        return Err(Error::Other("error: quiet_delta must be negative".into()));
    }
    let mut sum = 0.0f64;
    for &x in rms_db {
        sum = sum + from_db(x);
    }
    let thr = args.quiet_delta_db + (sum / rms_db.len() as u32 as f64).log10() * 20.0;
    let mut kept_idx = Vec::new();
    let mut kept_val = Vec::new();
    for (i, &v) in rms_db.iter().enumerate() {
        if v > thr {
            kept_idx.push(i as u32);
            // The values reached `segment` as default-formatted ostream text.
            kept_val.push(via_g6(v));
        }
    }
    let breakpoints = segment(&kept_val, args, progress)?;
    let hop = (sample_rate as f64 * args.bin_s) as i32 as u32;
    Ok(breakpoints.iter().map(|&b| kept_idx[b as usize].wrapping_mul(hop) as u64).collect())
}

/// Prefix sums of one chunk (CalculatorData0).
struct Sums {
    s: Vec<f64>,
    q: Vec<f64>,
}

impl Sums {
    fn new(x: &[f64]) -> Self {
        let mut s = Vec::with_capacity(x.len());
        let mut q = Vec::with_capacity(x.len());
        let (mut a, mut b) = (0.0f64, 0.0f64);
        for (k, &v) in x.iter().enumerate() {
            if k == 0 {
                a = v;
                b = v * v;
            } else {
                a = a + v;
                b = b + v * v;
            }
            s.push(a);
            q.push(b);
        }
        Sums { s, q }
    }

    /// RangeSSD (0x4d81c): sum of squared deviations from the mean over i..=j.
    fn ssd(&self, i: usize, j: usize) -> f64 {
        let (s, q) = if i == 0 { (self.s[j], self.q[j]) } else { (self.s[j] - self.s[i - 1], self.q[j] - self.q[i - 1]) };
        q - (s * s) / ((j - i + 1) as u16 as f64)
    }
}

fn clamp_maxbp(maxbp: u16, chunk: u16, minseg: u16) -> u16 {
    let lim = (chunk / minseg).wrapping_sub(1);
    if maxbp == 0 { lim } else { maxbp.min(lim) }
}

/// Breakpoints (segment start indices into `x`, chunk starts included, 0 omitted).
pub fn segment(x: &[f64], args: &SegmentArgs, progress: &mut Progress) -> Result<Vec<u32>, Error> {
    let n_total = x.len() as u32;
    let minseg = args.min_segment_length.max(1);
    let tol = args.tolerance;
    let c: u16 = if (args.chunk_size as u32) <= n_total { args.chunk_size } else { n_total as u16 };
    let (nchunks, mut chunk) = if c == 0 { (1u32, n_total as u16) } else { ((n_total - 1 + c as u32) / c as u32, c) };
    let mut maxbp = clamp_maxbp(args.max_breakpoints, chunk, minseg);

    let mut out = Vec::new();
    let mut best = vec![0.0f64; chunk as usize];
    let mut best_i: Vec<Vec<u16>> = Vec::new();
    progress.update(35.0, 69.0, 0.0)?;
    let mut offset: u32 = 0;
    for ci in 0..nchunks {
        if ci == nchunks - 1 && chunk != 0 {
            let r = (n_total % chunk as u32) as u16;
            if r != 0 {
                chunk = r;
            }
            maxbp = clamp_maxbp(maxbp, chunk, minseg);
        }
        if chunk < minseg {
            break;
        }
        let n = chunk as usize;
        let m = minseg as usize;
        let sums = Sums::new(&x[offset as usize..offset as usize + n]);
        best_i.resize(maxbp as usize + 1, Vec::new());

        let mut err = (sums.ssd(0, n - 1) / n as f64).sqrt();
        let mut k: u16 = 0;
        while k <= maxbp && err > tol {
            if k == 0 {
                for j in (m - 1)..n {
                    best[j] = sums.ssd(0, j);
                }
            } else {
                let bi = &mut best_i[k as usize];
                bi.clear();
                bi.resize(n, 0);
                let lo = ((k as u32 + 1) * minseg as u32 - 1) as u16 as usize;
                let mut j = n;
                while j > lo {
                    j -= 1;
                    // dp_step (0x4d892): smallest i wins ties.
                    let mut have = false;
                    let (mut bv, mut bidx) = (0.0f64, 0u16);
                    let mut i = k as i64 * m as i64 - 1;
                    while i < j as i64 - m as i64 + 1 {
                        let cst = best[i as usize] + sums.ssd(i as usize + 1, j);
                        if !have || bv > cst {
                            bv = cst;
                            bidx = i as u16;
                        }
                        have = true;
                        i += 1;
                    }
                    best[j] = bv;
                    bi[j] = bidx;
                }
            }
            err = (best[n - 1] / n as f64).sqrt();
            k += 1;
        }
        let nbp = k.saturating_sub(1) as usize;

        let mut gl = vec![0u32; nbp + 1];
        gl[0] = offset;
        let mut j = n - 1;
        for kk in (1..=nbp).rev() {
            let i = best_i[kk][j] as usize;
            gl[kk] = offset + 1 + i as u32;
            j = i;
        }
        out.extend(gl.into_iter().filter(|&g| g != 0));
        offset += n as u32;
        progress.update(35.0, 69.0, if nchunks == 1 { 1.0 } else { ci as f64 / (nchunks - 1) as f64 })?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn finds_step_changes() {
        let mut x = vec![-30.0; 100];
        x.extend(vec![-10.0; 100]);
        x.extend(vec![-25.0; 100]);
        let cancel = AtomicBool::new(false);
        let mut cb = |_| {};
        let mut p = Progress::new(&mut cb, &cancel);
        let bps = segment(&x, &SegmentArgs::default(), &mut p).unwrap();
        assert_eq!(bps, vec![100, 200]);
    }

    #[test]
    fn flat_input_has_no_breakpoints() {
        let x = vec![-20.0; 500];
        let cancel = AtomicBool::new(false);
        let mut cb = |_| {};
        let mut p = Progress::new(&mut cb, &cancel);
        assert!(segment(&x, &SegmentArgs::default(), &mut p).unwrap().is_empty());
    }
}
