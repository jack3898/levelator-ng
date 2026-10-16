# Levelator (Rust)

A cross-platform Rust reimplementation of **The Levelator 2.1.2**, the
classic "drop a podcast on it and every voice comes out at the same loudness"
tool from The Conversations Network. The original was a Mac/Windows/Linux app
built on Python 2.6, wxPython and a closed-source C++ engine, and it no
longer runs on modern systems.

This port was built by reverse engineering the original `level` engine binary
with [REA](https://github.com/morluto/rea) and Ghidra. See
[docs/reverse-engineering](docs/reverse-engineering/README.md) for the full
analysis and per-stage specs.

It runs on **Windows, macOS and Linux**, and adds a few things the original
never had: background-noise reduction, and support for FLAC, MP3, M4A and
Ogg Vorbis besides WAV/AIFF.

## Download

Prebuilt binaries are attached to each
[GitHub Release](https://github.com/jack3898/levelator-reverse-engineer/releases):

| Platform | Archive | Contents |
|---|---|---|
| Windows x86_64 | `levelator-<tag>-windows-x86_64.zip` | `levelator.exe` (GUI), `level.exe` (CLI) |
| macOS Apple Silicon / Intel | `levelator-<tag>-macos-{arm64,x86_64}.tar.gz` | `Levelator.app`, `level` |
| Linux x86_64 | `levelator-<tag>-linux-x86_64.tar.gz` | `levelator`, `level` |

The macOS app is ad-hoc signed, not notarised. On first launch, right-click it
and choose **Open**.

## Usage

### Desktop app

Run `levelator`, then drag audio files onto the window. You can also
double-click the window or press Ctrl/Cmd+O to pick files. Each file is
levelled in turn and written next to the original as `name.output.<ext>`.
The **Noise reduction** and **Output** pickers apply to files added after
you change them.
Files passed on the command line (`levelator a.wav b.aiff`) are processed and
the app then quits, as in the original's batch mode.

### Command line

```sh
level talk.wav                       # writes talk.output.wav
level talk.mp3                       # writes talk.output.mp3 (192 kbit/s)
level talk.wav -f flac               # convert: writes talk.output.flac
level talk.m4a -f mp3 --mp3-bitrate 128
level talk.wav --denoise strong      # off | mild (default) | medium | strong
level talk.wav --denoise off         # exactly the original Levelator processing
level talk.wav out.wav -b 16         # force 16-bit output
level --target-level -16 talk.wav    # louder target (default -18 dB)
```

`level` prints `NN%` progress on stdout like the original engine. It also
accepts and ignores the original's third "key" argument, so old wrappers keep
working.

### Formats

| Format | Read | Write | Notes |
|---|---|---|---|
| WAV, AIFF / AIFF-C | ✓ | ✓ | 8/16/24/32-bit PCM or 32-bit float |
| FLAC | ✓ | ✓ | Lossless; written at 16 or 24 bits |
| MP3 | ✓ | ✓ | Written with LAME, constant bitrate (`--mp3-bitrate`, default 192 kbit/s), gapless |
| Ogg Vorbis | ✓ | ✓ | Quality-based VBR (`--ogg-quality`, default 0.5 ≈ 160 kbit/s) |
| M4A / MP4 (AAC, ALAC) | ✓ | — | No AAC encoder: AAC is written as MP3, ALAC as FLAC (or pick a format) |
| Opus | — | — | Not supported yet |

By default the output uses the input's format. Mono and stereo files are
supported, as in the original.

### Noise reduction

Before levelling, a gentle spectral (Wiener) filter turns down steady
background noise such as hiss, static, fans or air conditioning. It learns
the noise from the pauses in the recording and never cuts more than 12 dB at
the default **mild** setting (18 dB **medium**, 24 dB **strong**), so voices
keep their natural sound. Files with no real pauses, or with already silent
pauses, are left untouched. Use `--denoise off` (or **Off** in the app) for
exactly the original Levelator's processing.

In a test with a hiss floor of -50 dBFS, mild lowered the hiss in the pauses
by about 12 dB and changed the speech level by less than 0.1 dB.

## How it works

After the optional noise reduction, ten stages run, each a port of one of
the original's sub-programs:

1. **High-pass** at 62.5 Hz (2nd-order, 2-octave bandwidth) to remove rumble
2. **Noise gate**: −12 dB downward gate with 250 ms look-ahead; threshold set from the file's own level
3. **Peak reduction**: limits 50 ms bins whose peak is more than 15 dB above the mean
4. **Loudness reduction**: limits bins whose RMS is more than 3 dB above the mean (150 ms release)
5. **RMS analysis** in 50 ms bins
6. **Segmentation** of the loudness curve into steady sections (dynamic programming)
7. **Per-segment gains** toward the average level, soft-limited to ±24 dB
8. **Gain smoothing**: gain rises over 2 s; drops start slightly early
9. **Apply gains**, with per-sample interpolation
10. **Normalise** active speech to −18 dB RMS with peaks limited to −1 dBFS

The arithmetic follows the original operation by operation, including
libsndfile 1.0.21's sample-format rounding between stages.

## Building from source

```sh
cargo build --release          # target/release/level and target/release/levelator
cargo test --workspace
```

A C compiler is needed for the bundled LAME (MP3) and libvorbis sources. On
macOS and Linux, LAME is built with its own `./configure`, so `make` is
needed too.

On Linux the GUI needs the usual X11/Wayland and OpenGL libraries at runtime.
To build it you need `libxkbcommon-dev` and `libgtk-3-dev` (used by the file
dialog).

| Crate | Purpose |
|---|---|
| `crates/levelator-core` | The engine: audio I/O, codecs, noise reduction and the ten-stage pipeline |
| `crates/level-cli` | `level` command-line tool |
| `crates/levelator-gui` | `levelator` drag-and-drop desktop app (egui) |

## Releasing

Push a version tag and the [release workflow](.github/workflows/release.yml)
builds every platform and publishes a GitHub Release with the archives:

```sh
git tag v2.2.0
git push origin v2.2.0
```

You can also run it from the Actions tab: **Release → Run workflow**, pick a
branch and enter a new tag name. The workflow builds that branch and creates
the tag and the release.

## Disclaimer

This is an independent, clean-room reimplementation for interoperability and
preservation. It is not affiliated with or endorsed by The Conversations
Network, GigaVox Media or Singular Software. "The Levelator" is their name for
the original product. No code or assets from the original are included.

MP3 encoding uses [LAME](https://lame.sourceforge.io/) (LGPL), statically
linked; its source is in the `mp3lame-sys` crate, and this program's full
source is in this repository, so the binaries can be rebuilt against a
modified LAME. Ogg Vorbis encoding uses libvorbis/aoTuV (BSD).
