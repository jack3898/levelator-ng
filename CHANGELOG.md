# Changelog

## 2.2.0

New features (not in the original Levelator):

- **Noise reduction**: mild by default (`--denoise off|mild|medium|strong`,
  or the app's dropdown). It turns down steady hiss, static and hum, learned
  from the pauses, by at most 12/18/24 dB. `off` gives the original
  Levelator's processing.
- **More formats**: reads FLAC, MP3, M4A (AAC/ALAC) and Ogg Vorbis, and
  writes FLAC, MP3 and Ogg Vorbis, as well as WAV and AIFF. Output defaults
  to the input's format (AAC → MP3, ALAC → FLAC); `--format` or the app's
  **Output** dropdown converts.
- CLI options `--format`, `--mp3-bitrate`, `--ogg-quality`.

## 2.1.2

- First release: a Rust port of The Levelator 2.1.2 engine, reverse
  engineered from the original binary, with a `level` CLI and a drag-and-drop
  app for Windows, macOS and Linux.
