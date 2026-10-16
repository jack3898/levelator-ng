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

use clap::Parser;
use levelator_core::{level_file, output_path_for, Settings};

#[derive(Parser, Debug)]
#[command(name = "level", version, about = "Level the loudness of spoken-word WAV/AIFF recordings (Levelator 2.1.2 engine)")]
struct Cli {
    /// Input WAV or AIFF file
    input: PathBuf,
    /// Output file (default: <input>.output.<ext>)
    output: Option<PathBuf>,
    /// Ignored; the original GUI passed a time-based MD5 key here
    #[arg(hide = true)]
    key: Option<String>,
    /// Target loudness of the output in dB (RMS of active speech)
    #[arg(long, default_value_t = Settings::default().target_level_db, allow_hyphen_values = true)]
    target_level: f64,
    /// Don't print progress
    #[arg(short, long)]
    quiet: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = cli.output.clone().unwrap_or_else(|| output_path_for(&cli.input));
    let settings = Settings { target_level_db: cli.target_level, ..Settings::default() };
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
        Ok(()) => {
            if !quiet {
                println!();
                eprintln!("{} -> {} ({:.1} s)", cli.input.display(), output.display(), start.elapsed().as_secs_f64());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
