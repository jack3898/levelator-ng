//! Step 8: `smooth_gains` (0x5117a, ramp helper 0x50efc "SmoothGains1").
//!
//! Rate-limits gain *increases* (a rising step takes 40 bins = 2 s) while
//! letting decreases through; a second pass over the reversed sequence with
//! a 2-bin ramp makes decreases start slightly early. The result is the
//! element-wise minimum of the two passes.

#[derive(Clone, Debug)]
pub struct SmoothArgs {
    /// Ramp length (bins) for rising gain.
    pub attack_increasing: i32,
    /// Ramp length (bins) for falling gain, applied ahead of time.
    pub attack_decreasing: i32,
}

impl Default for SmoothArgs {
    fn default() -> Self {
        SmoothArgs {
            attack_increasing: 40,
            attack_decreasing: 2,
        }
    }
}

/// FUN_00050efc: follow `inp`, but approach higher values with a linear ramp.
fn ramp(inp: &[f64], attack: i32) -> Vec<f64> {
    let mut out = vec![0.0f64; inp.len()];
    if inp.is_empty() {
        return out;
    }
    out[0] = inp[0];
    let mut ramping = false;
    let mut step = 0.0f64;
    let mut count: i32 = 0;
    for i in 1..inp.len() {
        if ramping {
            let r = step + out[i - 1];
            if r > inp[i] {
                out[i] = inp[i];
                ramping = false;
            } else {
                out[i] = r;
                count -= 1;
                if count <= 0 {
                    ramping = false;
                }
            }
        } else {
            let x = inp[i];
            let p = out[i - 1];
            if x > p && attack != 1 {
                step = (x - p) / attack as f64;
                count = attack - 1;
                out[i] = p + step;
                ramping = true;
            } else {
                out[i] = x;
            }
        }
    }
    out
}

/// `gains` are the per-bin linear gains from calculate_gains. Exact zeros are
/// dropped, as the original's `atof(line) != 0` reader did.
pub fn run(gains: &[f64], args: &SmoothArgs) -> Vec<f64> {
    let g: Vec<f64> = gains.iter().copied().filter(|&v| v != 0.0).collect();
    let n = g.len();
    let fwd = ramp(&g, args.attack_increasing);
    let rev: Vec<f64> = g.iter().rev().copied().collect();
    let bwd = ramp(&rev, args.attack_decreasing);
    (0..n)
        .map(|i| {
            let b = bwd[n - 1 - i];
            if fwd[i] > b {
                b
            } else {
                fwd[i]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rises_slowly_falls_early() {
        let mut g = vec![1.0; 10];
        g.extend(vec![2.0; 50]);
        g.extend(vec![1.0; 10]);
        let s = run(
            &g,
            &SmoothArgs {
                attack_increasing: 4,
                attack_decreasing: 2,
            },
        );
        assert_eq!(&s[9..15], &[1.0, 1.25, 1.5, 1.75, 2.0, 2.0]);
        // The drop at index 60 starts one bin early (2-bin ramp, reversed).
        assert_eq!(&s[58..61], &[2.0, 1.5, 1.0]);
    }
}
