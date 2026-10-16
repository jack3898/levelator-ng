//! Full-pipeline checks on a synthetic two-speaker recording.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use levelator_core::audio::{self, AudioBuffer, AudioFormat, Container, Encoding};
use levelator_core::{level_file, Settings};

const SR: u32 = 22050;

/// Deterministic pseudo-random noise in [-1, 1).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

/// Voiced, syllable-modulated "speech" with an RMS of roughly `db` dBFS.
fn speech(out: &mut Vec<f64>, seconds: f64, db: f64, rng: &mut Lcg) {
    let amp = 10f64.powf(db / 20.0) * 2.0;
    let mut phase = 0.0f64;
    for i in 0..(seconds * SR as f64) as usize {
        let t = i as f64 / SR as f64;
        let env = (2.0 * std::f64::consts::PI * 4.0 * t).sin().powi(2);
        phase += 2.0 * std::f64::consts::PI * 150.0 / SR as f64;
        let voiced: f64 = (1..6).map(|k| (k as f64 * phase).sin() / k as f64).sum();
        out.push(amp * env * (0.5 * voiced + 0.1 * rng.next()));
    }
}

fn hiss(out: &mut Vec<f64>, seconds: f64, rng: &mut Lcg) {
    for _ in 0..(seconds * SR as f64) as usize {
        out.push(0.001 * rng.next());
    }
}

fn rms_db(x: &[f64]) -> f64 {
    20.0 * (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64)
        .sqrt()
        .log10()
}

fn interview() -> Vec<f64> {
    let mut rng = Lcg(7);
    let mut x = Vec::new();
    hiss(&mut x, 1.0, &mut rng);
    speech(&mut x, 8.0, -36.0, &mut rng); // 1..9 s   quiet guest
    hiss(&mut x, 1.0, &mut rng);
    speech(&mut x, 8.0, -20.0, &mut rng); // 10..18 s loud host
    hiss(&mut x, 1.0, &mut rng);
    speech(&mut x, 8.0, -36.0, &mut rng); // 19..27 s quiet guest
    x
}

fn run(container: Container, encoding: Encoding, channels: u16) {
    let mono = interview();
    let format = AudioFormat {
        container,
        encoding,
        sample_rate: SR,
        channels,
    };
    let data = mono
        .iter()
        .flat_map(|&v| std::iter::repeat_n(v, channels as usize))
        .collect();
    let ext = if container == Container::Wav {
        "wav"
    } else {
        "aiff"
    };
    let dir = std::env::temp_dir();
    let input = dir.join(format!("levelator-e2e-{encoding:?}-{channels}.{ext}"));
    let output: PathBuf = levelator_core::output_path_for(&input);
    audio::write(&input, &AudioBuffer { format, data }).unwrap();

    let cancel = AtomicBool::new(false);
    let mut last = 0;
    let mut progress = |p: u32| {
        assert!(p >= last, "progress went backwards");
        last = p;
    };
    level_file(
        &input,
        &output,
        &Settings::default(),
        &mut progress,
        &cancel,
    )
    .unwrap();
    assert_eq!(last, 100);

    let out = audio::read(&output).unwrap();
    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
    assert_eq!(out.format, format, "output keeps the input format");

    let left: Vec<f64> = out
        .data
        .iter()
        .step_by(channels as usize)
        .copied()
        .collect();
    let sec = |a: f64, b: f64| &left[(a * SR as f64) as usize..(b * SR as f64) as usize];
    let quiet = rms_db(sec(2.0, 8.0));
    let loud = rms_db(sec(11.0, 17.0));
    let input_gap = 16.0;
    // The 16 dB gap between the speakers is mostly closed and both sit near -18 dB.
    assert!(
        (quiet - loud).abs() < input_gap / 3.0,
        "quiet {quiet:.1} dB vs loud {loud:.1} dB"
    );
    assert!(
        (-24.0..-14.0).contains(&quiet),
        "quiet speaker at {quiet:.1} dB"
    );
    assert!(
        (-24.0..-14.0).contains(&loud),
        "loud speaker at {loud:.1} dB"
    );
    let peak = left.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(peak <= 1.0);
}

#[test]
fn levels_stereo_wav16() {
    run(Container::Wav, Encoding::Pcm(16), 2);
}

#[test]
fn levels_mono_aiff24() {
    run(Container::Aiff, Encoding::Pcm(24), 1);
}

#[test]
fn passes_short_files_through_unchanged() {
    let input = std::env::temp_dir().join("levelator-e2e-short.wav");
    let output = levelator_core::output_path_for(&input);
    let format = AudioFormat {
        container: Container::Wav,
        encoding: Encoding::Pcm(16),
        sample_rate: SR,
        channels: 1,
    };
    let data = (0..100).map(|i| (i as f64 * 0.1).sin() * 0.3).collect();
    audio::write(&input, &AudioBuffer { format, data }).unwrap();
    let cancel = AtomicBool::new(false);
    level_file(&input, &output, &Settings::default(), &mut |_| {}, &cancel).unwrap();
    assert_eq!(
        std::fs::read(&input).unwrap(),
        std::fs::read(&output).unwrap()
    );
    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}
