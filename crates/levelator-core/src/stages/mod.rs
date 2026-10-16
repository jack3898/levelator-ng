//! The individual processing stages of the original `level` pipeline.
//!
//! In the original every stage was a separate sub-program that read one
//! temporary sound file and wrote the next. Here a stage takes and returns a
//! [`Signal`]; [`Signal::requantize`] reproduces the rounding that writing
//! and re-reading an integer PCM temp file through libsndfile caused.

pub mod calc_rms;
pub mod calculate_gains;
pub mod gate;
pub mod highpass;
pub mod reduce_loud;
pub mod segment;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::audio::{self, AudioBuffer, AudioFormat};
use crate::Error;

/// Interleaved samples, as libsndfile's `sf_read_double` delivers them.
#[derive(Clone, Debug)]
pub struct Signal {
    pub data: Vec<f64>,
    pub channels: usize,
    pub sample_rate: i32,
    pub format: AudioFormat,
}

impl Signal {
    pub fn from_buffer(buf: &AudioBuffer) -> Self {
        let ch = buf.num_channels();
        let frames = buf.frames();
        let mut data = Vec::with_capacity(frames * ch);
        for i in 0..frames {
            for c in &buf.channels {
                data.push(c[i]);
            }
        }
        Signal { data, channels: ch, sample_rate: buf.format.sample_rate as i32, format: buf.format }
    }

    pub fn into_buffer(self) -> AudioBuffer {
        let ch = self.channels;
        let mut buf = AudioBuffer::new(self.format, self.frames());
        for (i, x) in self.data.into_iter().enumerate() {
            buf.channels[i % ch][i / ch] = x;
        }
        buf
    }

    pub fn frames(&self) -> usize {
        self.data.len() / self.channels.max(1)
    }

    /// Round every sample as a write + read of the stage's temp file would.
    pub fn requantize(&mut self) {
        let enc = self.format.encoding;
        for x in self.data.iter_mut() {
            *x = audio::quantize(*x, enc);
        }
    }
}

/// Progress reporting (0x5b166): each stage maps its own fraction onto a
/// slice of 0..100 and a new integer percentage is reported when it grows.
/// The shared cancel flag is checked on every update.
pub struct Progress<'a> {
    last: i64,
    callback: &'a mut dyn FnMut(u32),
    cancel: &'a AtomicBool,
}

impl<'a> Progress<'a> {
    pub fn new(callback: &'a mut dyn FnMut(u32), cancel: &'a AtomicBool) -> Self {
        Progress { last: -1, callback, cancel }
    }

    /// `b / (n - 1)`, or 1.0 when there is a single step.
    pub fn frac(b: usize, n: usize) -> f64 {
        if n > 1 { b as f64 / (n - 1) as f64 } else { 1.0 }
    }

    pub fn update(&mut self, start: f64, end: f64, frac: f64) -> Result<(), Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let pct = (start + (end - start) * frac).floor() as i64;
        if pct > self.last {
            self.last = pct;
            (self.callback)(pct.clamp(0, 100) as u32);
        }
        Ok(())
    }
}
