//! `level` — command-line Levelator.
//!
//! Drop-in for the original engine invocation `level <in> <out>`; the
//! front-end handshake key the original required as a third argument is accepted
//! and ignored. Progress is printed to stdout as `NN%` like the original, so
//! existing wrappers that parse it keep working.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::Parser;
use levelator_core::audio::{Container, EncodeOptions};
use levelator_core::denoise::Strength;
use levelator_core::{level_file, output_path_for, Settings};

#[derive(Parser, Debug)]
#[command(
    name = "level",
    version,
    about = "Level the loudness of spoken-word recordings (Levelator 2.1.2 engine)",
    after_help = "Reads WAV, AIFF, FLAC, MP3, M4A (AAC/ALAC) and Ogg Vorbis.\n\
                  Writes WAV, AIFF, FLAC, MP3 and Ogg Vorbis (M4A input is written as FLAC if ALAC, MP3 if AAC)."
)]
struct Cli {
    /// Input audio file
    input: PathBuf,
    /// Output file (default: <input>.output.<ext>)
    output: Option<PathBuf>,
    /// Ignored; the original GUI passed a time-based MD5 key here
    #[arg(hide = true)]
    key: Option<String>,
    /// Target loudness of the output in dB (RMS of active speech)
    #[arg(long, default_value_t = -18.0, allow_hyphen_values = true)]
    target_level: f64,
    /// Background noise reduction applied before levelling ("off" = original Levelator)
    #[arg(
        short,
        long,
        default_value = "mild",
        value_parser = PossibleValuesParser::new(["off", "mild", "medium", "strong"])
    )]
    denoise: String,
    /// Output format ("same" = like the input)
    #[arg(
        short,
        long,
        default_value = "same",
        value_parser = PossibleValuesParser::new(["same", "wav", "aiff", "flac", "mp3", "ogg"])
    )]
    format: String,
    /// Output bit depth for WAV/AIFF/FLAC: 0 keeps the input's, or 8, 16, 24, 32
    #[arg(
        short,
        long,
        default_value = "0",
        value_parser = PossibleValuesParser::new(["0", "8", "16", "24", "32"]).map(|s| s.parse::<u16>().unwrap())
    )]
    bit_depth: u16,
    /// MP3 bitrate in kbit/s (constant bitrate)
    #[arg(
        long,
        default_value = "192",
        value_parser = PossibleValuesParser::new(["64", "96", "128", "160", "192", "224", "256", "320"]).map(|s| s.parse::<u32>().unwrap())
    )]
    mp3_bitrate: u32,
    /// Ogg Vorbis quality from -0.1 to 1.0 (0.5 ≈ 160 kbit/s)
    #[arg(long, default_value_t = 0.5, allow_hyphen_values = true)]
    ogg_quality: f32,
    /// Don't print progress
    #[arg(short, long)]
    quiet: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = cli
        .output
        .clone()
        .unwrap_or_else(|| output_path_for(&cli.input));
    let settings = Settings {
        target_level_db: cli.target_level,
        bit_depth: (cli.bit_depth != 0).then_some(cli.bit_depth),
        noise_reduction: Strength::from_name(&cli.denoise),
        output_format: Container::from_name(&cli.format),
        encode: EncodeOptions {
            mp3_kbps: cli.mp3_bitrate,
            vorbis_quality: cli.ogg_quality,
        },
    };
    let cancel = AtomicBool::new(false);
    let quiet = cli.quiet;
    let mut last = u32::MAX;
    let mut progress = |p: u32| {
        if !quiet && p != last {
            last = p;
            print!("{p}%\r");
            let _ = std::io::stdout().flush();
        }
    };
    let start = std::time::Instant::now();
    match level_file(&cli.input, &output, &settings, &mut progress, &cancel) {
        Ok(written) => {
            if !quiet {
                println!();
                eprintln!(
                    "{} -> {} ({:.1} s)",
                    cli.input.display(),
                    written.display(),
                    start.elapsed().as_secs_f64()
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
