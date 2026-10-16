# Reverse engineering The Levelator 2.1.2

These notes record how the Rust port in this repository was derived from
`Levelator-2.1.2.dmg` (macOS, 2010). The per-stage specs in this folder are
the reference the Rust code was written against. Each one cites function
addresses in the i386 slice of the `level` binary.

No code or assets from the original are included in this repository.

## What is in the DMG

`Levelator.app` is a py2app bundle:

| Part | What it is |
|---|---|
| `Contents/Resources/lib/python2.6/site-packages.zip` → `gui/`, `proj/`, `utl/` | Python 2.6 + wxPython front-end (decompiled with `uncompyle6`) |
| `Contents/Resources/osx/level` | The actual engine: a universal i386/PPC C++ executable (boost, libsndfile 1.0.21, Crypto++) with C++ symbols mostly intact |
| `Contents/Resources/osx/libsndfile.1.dylib` | libsndfile 1.0.21 |

The front-end runs `level "<in>" "<out>" <key>` in a subprocess and parses
`NN%` progress from its stdout. The output name is the input with `.output`
inserted before the extension. The `<key>` is
`md5("The Levelator copyright (c) 2006 by GigaVox Media, Inc. and Singular Software" + str(floor(time/10)*10))`,
a handshake so only the GUI can drive the engine. The port drops it.

## The engine pipeline

`main` → `FUN_00028dfc` ([orchestrator.md](orchestrator.md)) runs ten
sub-programs. Each one parsed its own argv with boost::program_options and
passed temp files to the next:

| # | Stage | Spec | What it does |
|---|---|---|---|
| 1 | highpass | [highpass.md](highpass.md) | RBJ 2nd-order HPF at 62.5 Hz with a 2-octave bandwidth (from `--low 25 --high 100`). Also returns the file's level in dB |
| 2 | gate | [gate.md](gate.md) | Downward noise gate (0 / −12 dB), 5 ms sum-of-squares meters, 250 ms look-ahead, hold 250 ms, attack 250 ms, release 750 ms. Threshold = `min(level−18, −40)` dB |
| 3 | reduce_loud_peak | [reduce_loud_peak.md](reduce_loud_peak.md) | 50 ms bins: limits bin *peaks* to 15 dB above the mean RMS of non-quiet bins, then peak-normalises |
| 4 | reduce_loud | [reduce_loud.md](reduce_loud.md) | Same structure: limits bin *RMS* to 3 dB above the mean, with a 150 ms release, then peak-normalises |
| 5 | calc_rms | [calc_rms.md](calc_rms.md) | dBFS RMS per 50 ms bin |
| 6 | wrap_segment / segment | [segment.md](segment.md) | Drops bins more than 12 dB below the mean, then DP-segments the dB curve into piecewise-constant runs (SSD cost, ≥20 bins per segment, ≤30 breakpoints per 2400-bin chunk, until the RMS residual is ≤ 3.5 dB) |
| 7 | calculate_gains | [calculate_gains.md](calculate_gains.md) | Gated RMS per segment. Moves each segment to the mean segment level, soft-limited to ±24 dB with `24·tanh(dev/24)` |
| 8 | smooth_gains | [smooth_gains.md](smooth_gains.md) | Rising gain ramps over 40 bins (2 s); falling gain starts 2 bins early |
| 9 | apply gains | [apply_gains.md](apply_gains.md) | Per-frame interpolated gain, then peak-normalise to 1.0 |
| 10 | normalize | [normalize.md](normalize.md) | Secant search for the broadband gain that puts active speech at −18 dB RMS. Bins are peak-limited to −1 dBFS with 2 s ramps |

Every audio hand-off went through a temp file in the input's own sample
format, so the samples were re-quantised by libsndfile 1.0.21 at each step.
The port reproduces this rounding without temp files (`Signal::requantize`).
Text hand-offs (`%14.12lf`, `%5.2lf`, `%.6g`) are reproduced the same way.

### Things that are compiled in but disabled

The binary also contains an `EncryptedLog` (Crypto++ RSA + AES), and code that
writes a `<levelLog>` XML file to `$HOME/log/level/` and uploads it with
`ping`/`wput` to an FTP server (credentials are embedded in plain text). A
compile-time switch disables all of this in 2.1.2, and none of it was ported.

## Tooling

- **[REA](https://github.com/morluto/rea)** (`rea-agents` 6.5.0) as an MCP
  server, with **Ghidra 12.1.4** as its native provider: `open_binary`,
  `batch_decompile`, `search_procedures`, `resolve_containing_procedure`, and
  so on. [`tools/mcpc.mjs`](tools/mcpc.mjs) is the small stdio MCP client used
  to keep one Ghidra session open across hundreds of calls.
- `llvm-objdump` for disassembly, plus [`tools/annotate.py`](tools/annotate.py)
  to resolve i386 PIC references (`call L; L: pop ebx; … [ebx+imm]`) to strings
  and floating-point constants. Ghidra's pseudocode often hides these.
- [`tools/stubs.py`](tools/stubs.py) to map symbol stubs to libm/libsndfile
  imports, and [`tools/split_fat.py`](tools/split_fat.py) to extract the i386
  slice.
- The defaults the pipeline passes to each stage are `std::string` globals
  built in `__StaticInit`. They were recovered by disassembling that section
  separately.

### Reproducing the analysis

```sh
7z x Levelator-2.1.2.dmg -oout                 # p7zip reads HFS+ DMGs
python3 tools/split_fat.py out/Levelator-2.1.2/Levelator.app/Contents/Resources/osx/level
llvm-objdump -d --no-show-raw-insn --x86-asm-syntax=intel level.i386 > level.i386.asm
python3 tools/annotate.py level.i386.asm level.i386 level.ann.asm

npm install rea-agents                          # REA CLI / MCP server
export GHIDRA_INSTALL_DIR=/path/to/ghidra_12.1.4_PUBLIC
cat > job.json <<'JSON'
{"calls":[{"name":"open_binary","args":{"path":"level.i386","provider_id":"ghidra"}},
          {"name":"batch_decompile","args":{"addresses":["0x28dfc","0x3d836"]},"out":"dec.json"}]}
JSON
node tools/mcpc.mjs node_modules/.bin/rea job.json
```

The specs refer to these local artifacts as `re/fn/<addr>.c` (one REA
`batch_decompile` result per function) and `bin/level.ann.asm`.

## Known limitations of the port

- The original Mac binary can't run on Linux CI, so outputs have **not** been
  compared sample-by-sample with Levelator 2.1.2. Fidelity rests on the
  static analysis in these specs.
- macOS libm `sin`/`cos`/`sinh`/`pow`/`log10` may differ from Rust's by 1 ulp.
- Where the original would have overflowed a PCM sample (libsndfile 1.0.21
  wraps), the port clips.
- The original's normalize step could loop forever when the measured level
  stops responding to gain. The port stops after 1000 secant iterations.
- The whole file is held in memory as `f64` (about 1.3 GB per hour of
  stereo 44.1 kHz audio). The original streamed through temp files.
