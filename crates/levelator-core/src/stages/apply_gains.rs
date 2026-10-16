//! Step 9: apply gains (0x2f9b8, `apply_gains/common.h`).
//!
//! Multiplies the signal by the per-bin gain curve (linearly interpolated
//! across each 50 ms bin, fading to 0 over the last bin) and scales the
//! result so its absolute peak is exactly 1.0.

use super::{Progress, Signal};
use crate::Error;

pub fn run(
    mut sig: Signal,
    gains: &[f64],
    bin_ms: i32,
    progress: &mut Progress,
) -> Result<Signal, Error> {
    let mut g = gains.to_vec();
    g.push(0.0);
    let bf = (sig.sample_rate * bin_ms) / 1000;
    if bf <= 0 {
        return Err(Error::Format("sample rate too low".into()));
    }
    let ch = sig.channels;
    let bin_items = bf as usize * ch;
    let nb = g.len() - 1;
    let len = sig.data.len();
    let block = |i: usize| (i * bin_items).min(len)..((i + 1) * bin_items).min(len);

    // Pass 1: peak of the gained signal.
    let mut peak = 0.0f64;
    for i in 0..nb {
        let mut a = g[i];
        let d = (g[i + 1] - g[i]) / bf as f64;
        for fr in sig.data[block(i)].chunks(ch) {
            for &x in fr {
                let v = (a * x).abs();
                peak = if v > peak { v } else { peak };
            }
            a = a + d;
        }
    }

    // Pass 2: apply, normalising the peak to 1.0. Audio past the last gain
    // bin is dropped, as in the original.
    let mut written = 0;
    for i in 0..nb {
        let r = block(i);
        if r.is_empty() {
            break;
        }
        written = r.end;
        let mut a = g[i] / peak;
        let d = (g[i + 1] / peak - a) / bf as f64;
        for fr in sig.data[r].chunks_mut(ch) {
            for x in fr {
                *x = a * *x;
            }
            a = a + d;
        }
        let frac = if g.len() < 3 {
            1.0
        } else {
            i as f64 / (g.len() - 2) as f64
        };
        progress.update(82.0, 91.0, frac)?;
    }
    sig.data.truncate(written);
    let mut out = sig;
    out.requantize();
    Ok(out)
}
