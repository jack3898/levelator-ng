//! The ten-stage levelling pipeline (work in progress: stages are being ported
//! from the reverse-engineered `level` binary).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::audio::AudioBuffer;
use crate::Error;

/// User-tunable settings. Defaults are the values hard-coded in Levelator 2.1.2.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Final loudness target in dB (`normalize -l`).
    pub target_level_db: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { target_level_db: -18.0 }
    }
}

pub fn level_buffer(
    _input: &AudioBuffer,
    _settings: &Settings,
    _progress: &mut dyn FnMut(u32),
    _cancel: &AtomicBool,
) -> Result<AudioBuffer, Error> {
    Err(Error::Other("levelling engine not implemented yet".into()))
}

pub fn level_file(
    input: &Path,
    output: &Path,
    settings: &Settings,
    progress: &mut dyn FnMut(u32),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let buf = crate::audio::read(input)?;
    let out = level_buffer(&buf, settings, progress, cancel)?;
    crate::audio::write(output, &out)
}
