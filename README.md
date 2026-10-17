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

It runs on **Windows, macOS and Linux**. The levelling engine is a faithful
port, and a few extras the original never had are added on top: background
noise reduction, and support for FLAC, MP3, M4A and Ogg Vorbis besides
WAV/AIFF. **Noise reduction is on by default, so out of the box the output is
not identical to Levelator 2.1.2.** See
[how it compares to the original](#how-it-compares-to-the-original) and
[getting the original behaviour](#getting-the-original-behaviour).

## How it compares to the original

### The core: a faithful port

The ten-stage levelling engine (high-pass, noise gate, loudness reduction,
segmentation, per-segment gains, smoothing and normalisation) is ported
operation by operation from the original `level` binary. It uses the same
algorithms, the same hard-coded settings and the same order of floating-point
operations, and it reproduces the rounding the original's temporary files
caused between stages. The aim is that, for a WAV or AIFF file with the extras
switched off, the output matches what Levelator 2.1.2 produced.

That aim is not verified sample by sample. The original is a 2010 Mac binary
that can't be run alongside this port, so fidelity rests on the static
analysis documented in [docs/reverse-engineering](docs/reverse-engineering/README.md).

### Extras (not in the original)

| Extra | Default | Changes the sound? | Original behaviour |
|---|---|---|---|
| **Background noise reduction** before levelling | **On (mild)** | **Yes**: steady hiss and hum are turned down, by up to 12 dB at mild | `--denoise off`, or **Off** in the app |
| Reading FLAC, MP3, M4A (AAC/ALAC) and Ogg Vorbis | — | No: it only decodes them. The original accepted only WAV/AIFF and refused MP3 | Use WAV/AIFF input |
| Writing FLAC, MP3 and Ogg Vorbis, and converting between formats | Same format as the input | Only through the codec itself: MP3 and Ogg are lossy | Keep **Output: Same as input** (the default) |
| `--target-level`, `--bit-depth`, `--mp3-bitrate`, `--ogg-quality` options | The original's values (−18 dB, keep the input's bit depth) | Only if you change them | Leave them at their defaults |
| Noise-reduction and output-format pickers in the app | — | Through the settings above | — |

When noise reduction is on, it takes the first 10% of the progress bar, and
the original stages' percentages are scaled into the remaining 90%.

### Small deliberate differences in the core

These apply even with every extra switched off:

- **Clipping instead of wrap-around.** If a sample would exceed full scale
  when written, it is clipped. The original's libsndfile 1.0.21 wrapped it
  around, producing a loud click. The original's own peak limiting means
  this almost never happens.
- **No endless loop.** The original's final normalise step could hang forever
  on unusual input. The port stops after 1000 iterations and keeps the
  last result.
- **In memory, not temp files.** The original wrote ten temporary files to
  disk. The port keeps everything in memory but rounds the samples and
  numbers exactly as those files did.
- **Output naming fix.** `talk.wav` → `talk.output.wav` as before. The
  original split the whole path on `.`, which broke for folders with a dot in
  their name. The port only looks at the file name.
- **No licence handshake.** The original GUI passed the engine a time-based
  MD5 "key" so nothing else could run it. `level` accepts that argument and
  ignores it.
- **No network access.** The original app checked conversationsnetwork.org
  for updates and news. That site is gone, so this was dropped. Usage logging
  and FTP upload code was compiled into the original but disabled, and it was
  not ported.
- **Floating-point libraries.** `sin`, `cos`, `pow` and `log10` on modern
  systems can differ from 2010's macOS in the last bit. The effect is far
  below anything audible.

### Getting the original behaviour

- **App:** set **Noise reduction** to **Off**, keep **Output** at **Same as
  input**, and use WAV or AIFF files.
- **Command line:** `level --denoise off talk.wav`

## Download

Prebuilt binaries are attached to each
[GitHub Release](https://github.com/jack3898/levelator-ng/releases):

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

### NixOS / Nix

The prebuilt Linux binary can't find the Wayland/OpenGL libraries on NixOS,
because they are not in the standard paths. Use the flake instead, which
builds from source and links them in:

```sh
nix run github:jack3898/levelator-ng           # the app
nix run github:jack3898/levelator-ng#level -- talk.wav   # the command line
nix profile install github:jack3898/levelator-ng
```

To use the prebuilt binary anyway, add the libraries to `nix-ld`:

```nix
programs.nix-ld.enable = true;
programs.nix-ld.libraries = with pkgs; [ wayland libxkbcommon libGL ];
```

`nix develop` gives a shell with the Rust toolchain and the libraries for
`cargo run`.

### Linux desktop requirements

The `levelator` app opens its window through Wayland or X11, and loads the
needed libraries when it starts. Most desktops already have them. On a
minimal install or WSL, install them with:

```sh
# Debian / Ubuntu
sudo apt install libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 libx11-6 libxcursor1 libxrandr2 libxi6 libgl1 libegl1
# Fedora
sudo dnf install libwayland-client libxkbcommon libxkbcommon-x11 libX11 libXcursor libXrandr libXi mesa-libGL mesa-libEGL
# Arch
sudo pacman -S wayland libxkbcommon libxkbcommon-x11 libx11 libxcursor libxrandr libxi mesa
```

If `WAYLAND_DISPLAY` is set but Wayland can't be used (the library is
missing, or no compositor is running, as can happen under WSL), the app falls
back to X11 automatically when `DISPLAY` is set. To force X11 yourself, run
`env -u WAYLAND_DISPLAY levelator`.

The open-file dialog (double-click or Ctrl+O) uses the desktop's XDG portal.
Without one, as on some WSL setups, drag and drop and the `level` command line
still work. `level` needs none of these libraries.

### Command line

```sh
level talk.wav                       # writes talk.output.wav
level talk.mp3                       # writes talk.output.mp3 (192 kbit/s)
level talk.wav -f flac               # convert: writes talk.output.flac
level talk.m4a -f mp3 --mp3-bitrate 128
level talk.wav --denoise strong      # off | mild (default) | medium | strong
level talk.wav --denoise off         # original Levelator processing, no extras
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

### Noise reduction (extra)

This is not part of the original Levelator; see
[how it compares to the original](#how-it-compares-to-the-original).

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

First, if enabled, the noise reduction (an extra). Then the ten stages of
the original engine run, each a port of one of the original's sub-programs:

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

No extra development packages are needed on Linux: the GUI loads its display
libraries at runtime (see [Linux desktop requirements](#linux-desktop-requirements)).

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
