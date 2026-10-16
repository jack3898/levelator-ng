//! The orchestrator (`level.cpp`, FUN_00028dfc): runs the ten stages in order.
//!
//! The original passed temporary files between stages (`<tmp>_01.wav` …
//! `<tmp>_10.wav` plus text files). Here everything stays in memory, but each
//! audio hand-off is re-quantised to the input's sample format and each text
//! hand-off goes through the same number formatting, so the numbers match.
//!
//! Optional noise reduction (not in the original) runs on the decoded input
//! before the first stage.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::audio::{self, AudioBuffer, Container, EncodeOptions, Encoding};
use crate::denoise::{self, Strength};
use crate::dsp::via_fixed;
use crate::stages::{
    apply_gains, calc_rms, calculate_gains, gate, highpass, normalize, reduce_loud, segment,
    smooth_gains, Progress, Signal,
};
use crate::Error;

/// User-tunable settings. The levelling defaults are the values hard-coded
/// in Levelator 2.1.2.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Final loudness target in dB (`normalize -l`).
    pub target_level_db: f64,
    /// Output bit depth (8/16/24/32) for PCM outputs; `None` keeps the
    /// input's (the original's `--bit-depth 0`).
    pub bit_depth: Option<u16>,
    /// Background-noise reduction before levelling; `None` disables it and
    /// gives the original Levelator's processing.
    pub noise_reduction: Option<Strength>,
    /// Output format; `None` = same as the input (see [`audio::output_container`]).
    pub output_format: Option<Container>,
    /// MP3 / Ogg Vorbis encoder settings.
    pub encode: EncodeOptions,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            target_level_db: -18.0,
            bit_depth: None,
            noise_reduction: Some(Strength::Mild),
            output_format: None,
            encode: EncodeOptions::default(),
        }
    }
}

/// Inputs shorter than this many seconds are passed through untouched.
const MIN_DURATION_S: f64 = 0.05;

fn step(n: u32, r: Result<Signal, Error>) -> Result<Signal, Error> {
    r.map_err(|e| step_error(n, e))
}

fn step_error(n: u32, e: Error) -> Error {
    match e {
        Error::Cancelled => Error::Cancelled,
        e => Error::Other(format!("{e}\nStep {n} did not complete successfully")),
    }
}

/// The original copied the input unchanged when it was shorter than 50 ms
/// or every sample equalled the first frame (0x28a9c).
fn passthrough(buf: &AudioBuffer) -> bool {
    let duration = buf.frames() as f64 / buf.sample_rate() as f64;
    if !(duration >= MIN_DURATION_S) {
        return true;
    }
    let ch = buf.num_channels();
    let first = &buf.data[..ch.min(buf.data.len())];
    buf.data
        .chunks(ch)
        .all(|f| f.iter().zip(first).all(|(a, b)| a == b))
}

/// Run the pipeline on decoded audio that is not a [`passthrough`] case.
fn process(
    input: AudioBuffer,
    settings: &Settings,
    progress: &mut Progress,
    name: &str,
) -> Result<AudioBuffer, Error> {
    if !matches!(settings.bit_depth, None | Some(8 | 16 | 24 | 32)) {
        return Err(Error::Other(format!(
            "Unknown bit depth: {}",
            settings.bit_depth.unwrap_or(0)
        )));
    }
    let mut sig = Signal::from_buffer(input);
    let sample_rate = sig.sample_rate;

    // 0. Optional noise reduction gets the first 10% of the progress bar.
    if let Some(strength) = settings.noise_reduction {
        if sig.channels <= 2 {
            denoise::run(&mut sig, strength, progress)?;
            sig.requantize();
        }
        progress.set_range(10.0, 100.0);
    }

    // 1. High-pass; its level sets the gate threshold.
    let (sig, level) = highpass::run(sig, &highpass::HighpassArgs::default(), progress, name)
        .map_err(|e| step_error(1, e))?;
    if !(level < 0.0) {
        return Err(Error::Other("Step 1 did not complete successfully".into()));
    }
    let thr = {
        let t = level - 18.0;
        if -40.0 < t {
            -40.0
        } else {
            t
        }
    };
    let thr = via_fixed(thr, 1);

    // 2-4. Gate and loudness reduction.
    let sig = step(
        2,
        gate::run(sig, &gate::GateArgs::with_threshold(thr), progress, name),
    )?;
    let sig = step(
        3,
        reduce_loud::run(
            sig,
            &reduce_loud::ReduceLoudArgs::peak_defaults(),
            progress,
            name,
        ),
    )?;
    let sig = step(
        4,
        reduce_loud::run(
            sig,
            &reduce_loud::ReduceLoudArgs::rms_defaults(),
            progress,
            name,
        ),
    )?;

    // 5-8. Measure, segment, compute and smooth the per-bin gains.
    let rms = calc_rms::run(&sig, 50, progress).map_err(|e| step_error(5, e))?;
    let cues = segment::wrap_segment(
        &rms,
        sample_rate,
        &segment::SegmentArgs::default(),
        progress,
    )
    .map_err(|e| step_error(6, e))?;
    let (gains, _) =
        calculate_gains::run(&sig, &cues, &calculate_gains::GainArgs::default(), progress)
            .map_err(|e| step_error(7, e))?;
    let gains = smooth_gains::run(&gains, &smooth_gains::SmoothArgs::default());

    // 9-10. Apply and normalise to the target level.
    let sig = step(9, apply_gains::run(sig, &gains, 50, progress))?;
    let args = normalize::NormalizeArgs {
        target_level_db: settings.target_level_db,
        bit_depth: settings.bit_depth,
        ..normalize::NormalizeArgs::default()
    };
    // The original reported a normalize failure as "Step 9" too.
    let sig = step(9, normalize::run(sig, &args, progress, name))?;
    Ok(sig.into_buffer())
}

/// Level an in-memory buffer. A buffer the original would have passed
/// through untouched is returned as is.
pub fn level_buffer(
    input: AudioBuffer,
    settings: &Settings,
    progress: &mut dyn FnMut(u32),
    cancel: &AtomicBool,
) -> Result<AudioBuffer, Error> {
    let mut p = Progress::new(progress, cancel);
    if passthrough(&input) {
        p.update(100.0, 100.0, 1.0)?;
        return Ok(input);
    }
    process(input, settings, &mut p, "input")
}

/// Level `input` and write the result next to `output`.
///
/// The output format is `settings.output_format`, else the format implied by
/// `output`'s extension, else the input's. If that format differs from the
/// extension of `output` (e.g. an AAC input written as MP3), the extension is
/// replaced. Returns the path actually written.
pub fn level_file(
    input: &Path,
    output: &Path,
    settings: &Settings,
    progress: &mut dyn FnMut(u32),
    cancel: &AtomicBool,
) -> Result<PathBuf, Error> {
    if !input.exists() {
        return Err(Error::OpenInput(format!(
            "Could not find {}",
            input.display()
        )));
    }
    let buf = audio::read(input)?;
    let requested = settings.output_format.or_else(|| {
        output
            .extension()
            .and_then(|e| e.to_str())
            .and_then(Container::from_name)
    });
    let container = audio::output_container(&buf.format, requested);
    let output = if output
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Container::from_name)
        == Some(container)
    {
        output.to_path_buf()
    } else {
        output.with_extension(container.extension())
    };

    let name = input.display().to_string();
    let mut p = Progress::new(progress, cancel);
    let mut out = if passthrough(&buf) {
        p.update(100.0, 100.0, 1.0)?;
        if container == buf.format.container {
            // The original copied the file byte for byte.
            drop(buf);
            let werr = |e: std::io::Error| Error::Write(format!("{}: {e}", output.display()));
            if output.exists() {
                std::fs::remove_file(&output).map_err(werr)?;
            }
            std::fs::copy(input, &output).map_err(werr)?;
            return Ok(output);
        }
        buf
    } else {
        process(buf, settings, &mut p, &name)?
    };

    // Converting a lossy (float-decoded) source to a PCM format: 16-bit
    // unless asked otherwise.
    if container != out.format.container {
        if let (Encoding::Float(_), Container::Wav | Container::Aiff | Container::Flac) =
            (out.format.encoding, container)
        {
            out.format.encoding = Encoding::Pcm(settings.bit_depth.unwrap_or(16));
        }
        out.format.container = container;
    }
    audio::write_with(&output, &out, &settings.encode)?;
    Ok(output)
}
