//! Step 5: `calc_rms` (0x3271e) — RMS level of consecutive 50 ms bins, in dBFS.

use super::{Progress, Signal};
use crate::dsp::via_fixed;
use crate::Error;

/// One dB value per bin, as `_05.txt` held them (`%14.12lf`, floored at -200).
pub fn run(sig: &Signal, bin_ms: u32, progress: &mut Progress) -> Result<Vec<f64>, Error> {
    let ch = sig.channels as u32;
    let mut bin = bin_ms.wrapping_mul(ch).wrapping_mul(sig.sample_rate as u32) / 1000;
    bin -= bin % ch;
    if bin == 0 {
        return Err(Error::Format("sample rate too low".into()));
    }
    let total = sig.data.len() as u64;
    let mut read_total: u64 = 0;
    let mut out = Vec::with_capacity(sig.data.len() / bin as usize + 1);
    for chunk in sig.data.chunks(bin as usize) {
        let n = chunk.len();
        read_total += n as u64;
        let mut sum = 0.0f64;
        for &x in chunk {
            sum = sum + x * x;
        }
        let db = 20.0 * (sum / n as f64).sqrt().log10();
        let v = if -200.0 > db { -200.0 } else { db };
        out.push(via_fixed(v, 12));
        progress.update(34.0, 35.0, read_total as f64 / total as f64)?;
    }
    Ok(out)
}
