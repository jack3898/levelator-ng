//! Reading and writing the non-WAV/AIFF formats.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use levelator_core::audio::{self, AudioBuffer, AudioFormat, Container, Encoding};
use levelator_core::{level_file, Settings};

const SR: u32 = 22050;
/// MP3 at 22.05 kHz is limited to 160 kbit/s; test the lossy codecs at a realistic rate.
const LOSSY_SR: u32 = 44100;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("levelator-codecs-{name}"))
}

/// 1 s stereo test signal: 440 Hz left, 660 Hz right, at -12 dBFS peak.
fn tone(container: Container, encoding: Encoding) -> AudioBuffer {
    tone_at(container, encoding, SR)
}

fn tone_at(container: Container, encoding: Encoding, sr: u32) -> AudioBuffer {
    let amp = 10f64.powf(-12.0 / 20.0);
    let data = (0..sr as usize)
        .flat_map(|i| {
            let t = i as f64 / sr as f64;
            let l = amp * (std::f64::consts::TAU * 440.0 * t).sin();
            let r = amp * (std::f64::consts::TAU * 660.0 * t).sin();
            [l, r]
        })
        .collect();
    AudioBuffer {
        format: AudioFormat {
            container,
            encoding,
            sample_rate: sr,
            channels: 2,
        },
        data,
    }
}

fn rms_db(x: &[f64]) -> f64 {
    10.0 * (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).log10()
}

#[test]
fn flac_is_lossless() {
    for bits in [16, 24] {
        let buf = tone(Container::Flac, Encoding::Pcm(bits));
        let path = tmp(&format!("{bits}.flac"));
        audio::write(&path, &buf).unwrap();
        let back = audio::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back.format, buf.format);
        assert_eq!(back.data.len(), buf.data.len());
        let expected: Vec<f64> = buf
            .data
            .iter()
            .map(|&x| audio::quantize(x, Encoding::Pcm(bits)))
            .collect();
        assert_eq!(
            back.data, expected,
            "{bits}-bit FLAC must round-trip exactly"
        );
    }
}

#[test]
fn lossy_formats_round_trip() {
    for container in [Container::Mp3, Container::Ogg] {
        let buf = tone_at(container, Encoding::Float(32), LOSSY_SR);
        let path = tmp(&format!("rt.{}", container.extension()));
        audio::write(&path, &buf).unwrap();
        let back = audio::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back.format.container, container);
        assert_eq!(
            (back.format.sample_rate, back.format.channels),
            (LOSSY_SR, 2)
        );
        // Gapless: the LAME tag / Ogg granule positions give the exact length.
        assert_eq!(back.frames(), buf.frames(), "{container:?}");
        let level = rms_db(&back.data);
        assert!(
            (level - rms_db(&buf.data)).abs() < 0.5,
            "{container:?}: level {level:.2} dB"
        );
    }
}

#[test]
fn decodes_m4a() {
    for (name, encoding) in [
        ("tone-aac.m4a", Encoding::Float(32)),
        ("tone-alac.m4a", Encoding::Pcm(16)),
    ] {
        let buf = audio::read(&fixture(name)).unwrap();
        assert_eq!(buf.format.container, Container::M4a);
        assert_eq!(buf.format.encoding, encoding, "{name}");
        assert_eq!(
            (buf.format.sample_rate, buf.format.channels),
            (SR, 1),
            "{name}"
        );
        assert!(
            (buf.frames() as i64 - (SR / 2) as i64).abs() < 2048,
            "{name}: {} frames",
            buf.frames()
        );
        // ffmpeg's sine source is -18 dBFS; at -12 dB more, the RMS is about -33 dBFS.
        let level = rms_db(&buf.data);
        assert!((level + 33.1).abs() < 1.0, "{name}: {level:.2} dB");
    }
}

#[test]
fn opus_is_reported_as_unsupported() {
    let err = audio::read(&fixture("tone.opus")).unwrap_err().to_string();
    assert!(err.contains("Opus"), "{err}");
}

#[test]
fn m4a_output_falls_back_to_flac_or_mp3() {
    let cancel = AtomicBool::new(false);
    let settings = Settings {
        noise_reduction: None,
        ..Settings::default()
    };
    for (name, ext) in [("tone-alac.m4a", "flac"), ("tone-aac.m4a", "mp3")] {
        let out = tmp(&format!("fallback-{name}"));
        // A constant tone is passed through (as the original did), so this
        // also covers converting on the pass-through path.
        let written = level_file(&fixture(name), &out, &settings, &mut |_| {}, &cancel).unwrap();
        assert_eq!(written.extension().unwrap(), ext);
        let back = audio::read(&written).unwrap();
        std::fs::remove_file(&written).ok();
        assert_eq!(back.format.container, Container::from_name(ext).unwrap());
    }
}

#[test]
fn converts_wav_to_every_writable_format() {
    let input = tmp("convert-src.wav");
    audio::write(&input, &tone(Container::Wav, Encoding::Pcm(16))).unwrap();
    let cancel = AtomicBool::new(false);
    for container in Container::WRITABLE {
        let settings = Settings {
            output_format: Some(container),
            ..Settings::default()
        };
        let written = level_file(
            &input,
            &tmp("convert-out.wav"),
            &settings,
            &mut |_| {},
            &cancel,
        )
        .unwrap();
        assert_eq!(written.extension().unwrap(), container.extension());
        let back = audio::read(&written).unwrap();
        std::fs::remove_file(&written).ok();
        assert_eq!(back.format.container, container);
        assert_eq!(back.format.channels, 2);
    }
    std::fs::remove_file(&input).ok();
}
