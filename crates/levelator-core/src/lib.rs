//! Clean-room Rust reimplementation of the Levelator 2.1.2 engine.

// The DSP code spells out `x = x + y` and negated comparisons on purpose: they
// mirror the original's operand order and NaN behaviour, which is what keeps
// the port numerically faithful.
#![allow(clippy::assign_op_pattern, clippy::neg_cmp_op_on_partial_ord)]

pub mod audio;
pub mod dsp;
pub mod stages;

use std::path::Path;

/// Errors reported by the engine. Messages follow the original `level` binary where it had one.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Can't open the source file: {0}")]
    OpenInput(String),
    #[error("{0}")]
    Format(String),
    #[error("Error writing to {0}")]
    Write(String),
    #[error("{0} is silent")]
    Silent(String),
    #[error("{0}")]
    Other(String),
    #[error("Program stopped")]
    Cancelled,
}

impl Error {
    pub(crate) fn open_input(path: &Path, e: std::io::Error) -> Self {
        Error::OpenInput(format!("{} ({e})", path.display()))
    }
}

mod pipeline;
pub use pipeline::{level_buffer, level_file, Settings};

use std::path::PathBuf;

/// Output naming used by the original front-end: `talk.wav` -> `talk.output.wav`.
///
/// The original split the whole path on '.', which misbehaves when a directory
/// name contains a dot; this only looks at the file name.
pub fn output_path_for(input: &Path) -> PathBuf {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new_name = match name.rfind('.') {
        Some(i) if i > 0 => format!("{}.output{}", &name[..i], &name[i..]),
        _ => format!("{name}.output"),
    };
    input.with_file_name(new_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names() {
        assert_eq!(
            output_path_for(Path::new("/x/talk.wav")),
            Path::new("/x/talk.output.wav")
        );
        assert_eq!(
            output_path_for(Path::new("a.b.aiff")),
            Path::new("a.b.output.aiff")
        );
        assert_eq!(
            output_path_for(Path::new("/d.ir/noext")),
            Path::new("/d.ir/noext.output")
        );
    }
}
