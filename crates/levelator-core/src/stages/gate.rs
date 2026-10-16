//! Step 2: `gate` (0x39f10 → 0x39840, CNoiseGate).
//!
//! A per-channel downward noise gate: the gain moves linearly between 1.0
//! (open) and -12 dB (closed). Levels are sums of squares over a 5 ms window.
//! Opening is driven by a meter that looks 250 ms ahead (so the gate is fully
//! open when speech starts); closing waits for a 250 ms hold and then ramps
//! down over 750 ms.

use super::{Progress, Signal};
use crate::dsp::from_db;
use crate::Error;

#[derive(Clone, Debug)]
pub struct GateArgs {
    pub attenuation_max_db: f64,
    pub attenuation_min_db: f64,
    pub threshold_attack_db: f64,
    pub threshold_release_db: f64,
    pub attack_ms: i32,
    pub release_ms: i32,
    pub hold_ms: i32,
    pub buffer_ms: i32,
    pub anticipate_ms: i32,
}

impl GateArgs {
    /// Pipeline values; both thresholds come from the highpass level.
    pub fn with_threshold(threshold_db: f64) -> Self {
        GateArgs {
            attenuation_max_db: -12.0,
            attenuation_min_db: 0.0,
            threshold_attack_db: threshold_db,
            threshold_release_db: threshold_db,
            attack_ms: 250,
            release_ms: 750,
            hold_ms: 250,
            buffer_ms: 5,
            anticipate_ms: 250,
        }
    }
}

/// Running sum of squares over a ring of N samples (CLevelMeter).
struct Meter {
    buf: Vec<f64>,
    cur: usize,
    sum: f64,
}

impl Meter {
    fn prefilled(d: &[f64]) -> Self {
        let mut sum = 0.0f64;
        for &x in d {
            sum = x * x + sum;
        }
        Meter { buf: d.to_vec(), cur: 0, sum }
    }

    fn push(&mut self, x: f64) -> f64 {
        let o = self.buf[self.cur];
        self.sum = self.sum - o * o;
        self.buf[self.cur] = x;
        self.cur += 1;
        self.sum = x * x + self.sum;
        if self.cur == self.buf.len() {
            self.cur = 0;
        }
        self.sum
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    Closed,
    Attack,
    Release,
    Hold,
}

struct Params {
    thr_a: f64,
    thr_r: f64,
    g_open: f64,
    g_closed: f64,
    up: f64,
    down: f64,
    hold_n: i32,
}

struct Channel {
    state: State,
    gain: f64,
    hold: i32,
    cur: Meter,
    ahead: Meter,
}

impl Channel {
    fn step(&mut self, cur: f64, ahd: f64, p: &Params) {
        match self.state {
            State::Open => {
                if cur < p.thr_r {
                    self.state = State::Hold;
                    self.hold = 0;
                }
            }
            State::Closed => {
                if ahd > p.thr_a {
                    self.state = State::Attack;
                }
            }
            State::Attack => {
                if ahd < p.thr_r {
                    self.state = State::Release;
                } else {
                    self.gain = self.gain + p.up;
                    if self.gain >= p.g_open {
                        self.state = State::Open;
                        self.gain = p.g_open;
                    }
                }
            }
            State::Release => {
                if ahd > p.thr_a {
                    self.state = State::Attack;
                } else {
                    self.gain = self.gain - p.down;
                    if self.gain <= p.g_closed {
                        self.state = State::Closed;
                        self.gain = p.g_closed;
                    }
                }
            }
            State::Hold => {
                if cur > p.thr_r {
                    self.state = State::Open;
                } else {
                    self.hold += 1;
                    if self.hold > p.hold_n {
                        self.state = State::Release;
                    }
                }
            }
        }
    }
}

pub fn run(sig: &Signal, args: &GateArgs, progress: &mut Progress, name: &str) -> Result<Signal, Error> {
    let sr = sig.sample_rate;
    let ch = sig.channels;
    let frames = sig.frames();
    let ms = |v: i32| sr.wrapping_mul(v) / 1000;
    let attack_n = ms(args.attack_ms);
    let release_n = ms(args.release_ms);
    let hold_n = ms(args.hold_ms);
    let antic_n = ms(args.anticipate_ms);
    let n = ms(args.buffer_ms).max(1) as usize;

    // The look-ahead handle is seeked `antic_n` frames in; if that fails the
    // original copied the input file unchanged.
    if antic_n < 0 || antic_n as usize > frames {
        return Ok(sig.clone());
    }
    if frames < n {
        return Err(Error::Format(format!("{name}: Expected more samples")));
    }

    let g_open = from_db(args.attenuation_min_db);
    let g_closed = from_db(args.attenuation_max_db);
    let energy = |db: f64| {
        let l = from_db(db);
        n as f64 * (l * l)
    };
    let p = Params {
        thr_a: energy(args.threshold_attack_db),
        thr_r: energy(args.threshold_release_db),
        g_open,
        g_closed,
        up: (g_open - g_closed) / attack_n as f64,
        down: (g_open - g_closed) / release_n as f64,
        hold_n,
    };

    let mut chans: Vec<Channel> = (0..ch.min(2))
        .map(|c| {
            let init: Vec<f64> = (0..n).map(|i| sig.data[i * ch + c]).collect();
            let cur = Meter::prefilled(&init);
            let ahead = Meter::prefilled(&init);
            let open = p.thr_a < ahead.sum || p.thr_a < cur.sum;
            Channel {
                state: if open { State::Open } else { State::Closed },
                gain: if open { g_open } else { g_closed },
                hold: 0,
                cur,
                ahead,
            }
        })
        .collect();

    let block = 8192 - 8192 % ch;
    let total = sig.data.len();
    let mut out = sig.clone();
    let mut buf_ahead = vec![0.0f64; block];
    let mut main_pos = 0usize;
    let mut ahead_pos = antic_n as usize * ch;
    let nblocks = total.div_ceil(block);
    let mut bi = 0;
    while main_pos < total {
        let n_main = block.min(total - main_pos);
        let n_ahead = block.min(total - ahead_pos);
        buf_ahead[..n_ahead].copy_from_slice(&sig.data[ahead_pos..ahead_pos + n_ahead]);
        ahead_pos += n_ahead;

        let mut i = 0;
        while i < n_main {
            for (c, chan) in chans.iter_mut().enumerate() {
                let x = sig.data[main_pos + i + c];
                let cur = chan.cur.push(x);
                let ahd = if n_ahead >= 1 { chan.ahead.push(buf_ahead[i + c]) } else { cur };
                chan.step(cur, ahd, &p);
                out.data[main_pos + i + c] = x * chan.gain;
            }
            i += ch;
        }
        main_pos += n_main;
        progress.update(10.0, 20.0, Progress::frac(bi, nblocks))?;
        bi += 1;
    }
    out.requantize();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioFormat, Container, Encoding};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn attenuates_quiet_passages() {
        let sr = 8000;
        let mut data = vec![0.0001; sr as usize * 2];
        for (i, x) in data.iter_mut().enumerate().skip(sr as usize / 2).take(sr as usize) {
            *x = 0.5 * (i as f64 * 0.3).sin();
        }
        let format = AudioFormat { container: Container::Wav, encoding: Encoding::Float(64), sample_rate: sr as u32, channels: 1 };
        let sig = Signal { data, channels: 1, sample_rate: sr, format };
        let cancel = AtomicBool::new(false);
        let mut cb = |_| {};
        let mut p = Progress::new(&mut cb, &cancel);
        let out = run(&sig, &GateArgs::with_threshold(-40.0), &mut p, "t").unwrap();
        // Start: closed (-12 dB).
        assert!((out.data[10] / sig.data[10] - from_db(-12.0)).abs() < 1e-12);
        // Middle of the loud part: fully open.
        assert!((out.data[8000] - sig.data[8000]).abs() < 1e-12);
    }
}
