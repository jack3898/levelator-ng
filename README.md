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

It runs on **Windows, macOS and Linux**.

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

Run `levelator`, then drag WAV or AIFF files onto the window. You can also
double-click the window or press Ctrl/Cmd+O to pick files. Each file is
levelled in turn and written next to the original as `name.output.wav`.
Files passed on the command line (`levelator a.wav b.aiff`) are processed and
the app then quits, as in the original's batch mode.

### Command line

```sh
level talk.wav                     # writes talk.output.wav
level talk.aiff levelled.aiff      # explicit output path
level talk.wav out.wav -b 16       # force 16-bit output
level --target-level -16 talk.wav  # louder target (default -18 dB)
```

`level` prints `NN%` progress on stdout like the original engine. It also
accepts and ignores the original's third "key" argument, so old wrappers keep
working.

Input and output: uncompressed WAV, AIFF and AIFF-C (8/16/24/32-bit PCM or
32-bit float), mono or stereo. The output keeps the input's format. MP3 isn't
supported, as in the original.

## How it works

Ten stages, each a port of one of the original's sub-programs:

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

On Linux the GUI needs the usual X11/Wayland and OpenGL libraries at runtime.
To build it you need `libxkbcommon-dev` and `libgtk-3-dev` (used by the file
dialog).

| Crate | Purpose |
|---|---|
| `crates/levelator-core` | The engine: audio I/O and the ten-stage pipeline |
| `crates/level-cli` | `level` command-line tool |
| `crates/levelator-gui` | `levelator` drag-and-drop desktop app (egui) |

## Releasing

Push a version tag and the [release workflow](.github/workflows/release.yml)
builds every platform and publishes a GitHub Release with the archives:

```sh
git tag v2.1.2
git push origin v2.1.2
```

You can also run it manually from the Actions tab (**Release → Run workflow**)
with an existing tag.

## Disclaimer

This is an independent, clean-room reimplementation for interoperability and
preservation. It is not affiliated with or endorsed by The Conversations
Network, GigaVox Media or Singular Software. "The Levelator" is their name for
the original product. No code or assets from the original are included.
