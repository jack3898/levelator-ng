# Step 3 — `reduce_loud_peak` (entry FUN_000491e8)

Status: fully traced from asm (bin/level.ann.asm) — all constants and loop bounds below are read from the
instructions, not from Ghidra's pseudocode (which hides `scoped_array::operator[]` indices).

## Call chain

| addr | role |
|---|---|
| 0x491e8 → 0x49192 | sub-program `main(argc, argv)`; returns 0 |
| 0x4903a → 0x483a4 | build `CCLArgs` (defaults, 0x49296) and parse argv with boost::program_options |
| 0x4a990 | `ReduceLoudOperation op(args); op.run();` |
| 0x496d2 | `ReduceLoudOperation` ctor: noise-floor sign fix + dB→linear conversions |
| 0x4a322 | `run()`: open input, size bins, measure, compute gains, write |
| 0x49d7e | pass 1: per-bin peak / RMS + mean RMS of non-quiet bins |
| 0x493ce | per-bin gain = thr/peak for loud bins; returns max post-gain peak |
| 0x494f0 (+0x493aa fill) | build per-boundary gain array with attack/release ramps |
| 0x4931c | scale every boundary gain by 1/maxPeak (peak-normalise to 1.0) |
| 0x49806 | pass 2: read, apply linearly interpolated gain per frame, write |
| 0x4913c | 64-bit ceil-div `(a + b - 1) / b` (via `__divdi3`) |
| 0x482fe | "Noise floor: / Level difference:" — only `snprintf`s into a local buffer when the global verbose flag (0x1f33ec) is set; the text is **never output**. No effect. |
| 0x4936e | prints a string if `verbosity > 0` (not called in the op chain) |
| 0x4b486 | NOT part of this stage (called from 0x4e01f / 0x4e95c in `segment`). |

## Arguments (`ReduceLoudArgs`, ctor 0x49296; option binding at 0x484ac..0x48687)

| offset | type | option | default (ctor) | pipeline value |
|---|---|---|---|---|
| 0x00 | f64 | `--noise-floor,-f` "noise floor (dB)" | -44.0 (0xc0460000) | "-44.0" |
| 0x08 | f64 | `--level_diff,-l` "maximum level above mean (dB)" | 15.0 (0x402e0000) | "15.0" |
| 0x10 | i32 | `--attack,-a` "attack (ms)" | 0 | "0" |
| 0x14 | i32 | `--release,-r` "release (ms)" | 0 | "0" |
| 0x18 | i32 | `--bin-size,-s` "bin size (ms)" | 50 | "50" |
| 0x1c | string | positional `sound-file-in` | | `<out>_02` |
| 0x20 | string | positional `sound-file-out` | | `<out>_03` |
| 0x24 | i32 | `--verbosity,-v` | 1 | "0" |

Orchestrator (FUN_00028dfc, 0x2ad56..0x2ae45) calls `FUN_000491e8(15, argv)` with
`reduce_loud_peak --level_diff 15.0 --attack 0 --release 0 --noise-floor -44.0 --bin-size 50 --verbosity 0 <in> <out>`
(argv strings are static std::strings built in the static initialiser: 0x1f140c="15.0", 0x1f1408="0",
0x1f1404="0", 0x1f1400="-44.0", 0x1f13fc="50", 0x1f13f8="0"). Missing in/out → help + `ss_command_line_error`.
Non-zero return / exception → "Step 3 did not complete successfully".

## Operation object layout (for reference)

0x28 noiseLin, 0x30 levelLin, 0x38 threshold, 0x40 maxPeak, 0x48 meanRms, 0x50 SNDFILE*, 0x58 binFrames,
0x5c binSamples, 0x60 nBins, 0x64 nBins+1, 0x68 CBin[nBins], 0x6c f64 gains[nBins+1], 0x70 SF_INFO
(0x70 frames i64, 0x78 samplerate, 0x7c channels, ...).
`CBin` = 24 bytes: +0 gain f64, +8 peak f64, +0x10 rms f64.

## Constructor (0x496d2)

```
if noise_floor_db > 0.0 {            // ucomisd vs 0.0, strictly greater
    noise_floor_db = -noise_floor_db; // xorpd sign bit
    eprint!("Warning: Specified noise floor was positive.  Value has been changed to {:.2}.\n", noise_floor_db); // to std::cerr (0x1f3018)
}
levelLin = fromDB(level_diff_db);     // 0x30
noiseLin = fromDB(noise_floor_db);    // 0x28
fromDB(x) = pow(10.0, x / 20.0)       // 0x563fc  (constant 20.0, base 10.0 = 0x40240000)
```
Pipeline values: levelLin = 10^(0.75) ≈ 5.623413251903491, noiseLin = 10^(-2.2) ≈ 0.006309573444801929.

## run() (0x4a322)

1. `sf_open(in, SFM_READ=0x10, &sfinfo)`; null → `ss_sf_openinput_error`.
2. `channels > 2` → `ss_sf_too_many_channels_error(in, channels)` (so only mono / stereo).
3. `binFrames = (samplerate * bin_size_ms) / 1000` (i32 multiply, truncating signed div).
   `binSamples = binFrames * channels`.
   `nBins = ceil(frames / binFrames)` = `(frames + binFrames - 1) / binFrames` in i64 (0x4913c), stored as i32.
   Allocate `CBin[nBins]`, `gains[nBins+1]`.
4. `meanRms = measure()` (0x49d7e); `sf_seek(file, 0, SEEK_SET)`.
5. If `meanRms == 0.0` (exactly; NaN passes) → `ss_sf_input_silent_error(in)` (message contains " is silent").
6. `threshold = meanRms * levelLin`  (0x38)
7. `compute_bin_gains()` (0x493ce) → maxPeak
8. `build_gain_curve(1.0)` (0x494f0)
9. `scale_gains(1.0)` (0x4931c)
10. `write()` (0x49806)

## Pass 1: measure (0x49d7e) — combined channels

Buffer of `binSamples` f64. For `b in 0..nBins`:
```
n = sf_read_double(file, buf, binSamples)   // ITEMS (interleaved samples), not frames
if n == 0 { throw ss_sf_unexpected_eof(in) }   // "Expected more samples"
peak = 0.0; sumsq = 0.0;
for k in 0..n {                 // all channels together, interleaved order
    a = |buf[k]|
    peak = if a > peak { a } else { peak }   // ucomisd a, peak; cmovbe -> keep peak when a <= peak
    sumsq = a*a + sumsq
}
bin[b].peak = peak
bin[b].rms  = sqrt(sumsq / (n as f64))      // combined RMS over all n samples (both channels)
bin[b].gain = 1.0
if !(noiseLin >= bin[b].rms) {              // i.e. rms > noiseLin  (jae skip)
    totalSumsq = totalSumsq + sumsq
    totalCount += n                          // u64
}
progress(20.0 → 24.0, frac = if nBins > 1 { b as f64 / (nBins-1) as f64 } else { 1.0 })
```
Return `meanRms = if totalCount == 0 { 0.0 } else { sqrt(totalSumsq / (totalCount as f64)) }`.
(The u64→f64 conversion is exact for realistic counts.)
So the "noise floor" only excludes quiet bins from the mean; they are still processed later.

## Per-bin gain (0x493ce)

```
maxPeak = 0.0
for i in 0..nBins {
    if bin[i].peak > threshold {             // ucomisd peak, thr ; jbe -> else-branch
        bin[i].gain = threshold / bin[i].peak
        p = bin[i].peak * bin[i].gain        // (≈ threshold)
        if p > maxPeak { maxPeak = p }
    } else {
        if bin[i].peak > maxPeak { maxPeak = bin[i].peak }   // gain stays 1.0
    }
}
```
**"Peak" variant: the loudness measure compared to the threshold is the bin PEAK** (|sample| max), while the
threshold itself is derived from the mean RMS: `threshold = meanRms * 10^(level_diff/20)`.

## Gain curve with attack/release ramps (0x494f0, called with unity = 1.0)

`gains` has nBins+1 entries = gain at bin boundaries (boundary i = start of bin i).
```
gains.fill(1.0)                                    // 0x493aa
i = 0
while i < nBins {
    att = attack_ms  / bin_size_ms                 // i32 truncating div, recomputed each iteration
    rel = release_ms / bin_size_ms
    g = bin[i].gain
    if g < 1.0 {                                   // ucomisd 1.0, g ; jbe skip
        // forward (release) ramp: boundaries i+1 ..= i+1+rel
        stepR = (1.0 - g) / ((rel + 1) as f64)
        end = min(rel + i + 2, nBins + 1)          // exclusive
        cur = g
        for j in (i+1)..end { gains[j] = min(cur, gains[j]); cur = stepR + cur; }
        // backward (attack) ramp: boundaries i, i-1, ..., i-att
        stepA = (1.0 - g) / ((att + 1) as f64)
        lower = max(i - att - 1, -1)               // exclusive lower bound
        cur = g
        j = i
        while j > lower { gains[j] = min(cur, gains[j]); cur = stepA + cur; j -= 1; }
    }
    i += 1
}
```
`min(cur, x)` is SSE `minsd cur, x` → `if cur < x { cur } else { x }`.
With the pipeline values (attack 0, release 0): att = rel = 0, so each loud bin just sets
`gains[i] = min(gains[i], g)` and `gains[i+1] = min(gains[i+1], g)` — no ramps.

## Normalisation (0x4931c, called with 1.0)

```
f = 1.0 / maxPeak          // computed once (divsd), then multiplied
for j in 0..=nBins { gains[j] = f * gains[j] }
```
So the stage also peak-normalises: the largest bin peak after per-bin gain is mapped to 1.0 (0 dBFS).
This can amplify (if no bin exceeds threshold and max peak < 1). maxPeak > 0 is guaranteed when meanRms > 0.
Note maxPeak is computed from the nominal per-bin gains; the interpolated curve is ≤ those, so the output peak ≤ 1.0.

## Pass 2: write (0x49806)

```
out = sf_open(outPath, SFM_WRITE=0x20, &sfinfo)    // SAME SF_INFO struct as input → same format/samplerate/channels
if null → ss_sf_openoutput_error(out)
buf = [f64; binSamples]
for b in 0..nBins {
    nf = sf_readf_double(in, buf, binFrames)       // FRAMES (i64)
    g    = gains[b]
    step = (gains[b+1] - gains[b]) / (binFrames as f64)   // divides by full binFrames even for the short last bin
    for f in 0..nf {                                // i64 counter
        for c in 0..channels { buf[f*ch + c] = g * buf[f*ch + c]; }   // channel counter is u16 (irrelevant)
        g = g + step                                // accumulated, not recomputed
    }
    if sf_writef_double(out, buf, nf) != nf → ss_sf_io_error(out)
    progress(24.0 → 27.0, frac as above)
}
```
No `sf_command` calls in this stage (no SFC_SET_CLIPPING / normalisation changes): libsndfile defaults apply
for both the double read (normalised to ±1) and the double write (scaled back to the file's sample format).
If the file is integer PCM, every stage re-quantises (port must replicate libsndfile's double→int conversion).

## Progress (FUN_0005b166)

Struct {start, end, last=-1, current=-1, callback...}; `pct = floor(start + (end - start) * frac)`; if `pct > last`
it prints `"\r" << pct << "%"` to stdout (or calls a callback) and records it. Throws `ss_cancel_exception` if the
global cancel flag (0x1f33bc) is set. Ranges: measure 20→24, write 24→27.
If global verbose flag (0x1f33ec) is set, FUN_00028c7c prints "elapsed time: %4.1f seconds" (clock()/100) — not in normal runs.

## Errors
- open input fail → ss_sf_openinput_error; channels>2 → ss_sf_too_many_channels_error;
  sf_read_double returns 0 before all bins read → ss_sf_unexpected_eof; meanRms==0 → ss_sf_input_silent_error;
  open output fail → ss_sf_openoutput_error; short write → ss_sf_io_error.
  Any of these makes the orchestrator report "Step 3 did not complete successfully".

## Rust-like core

```rust
fn reduce_loud_peak(inp: &[f64] /*interleaved*/, ch: usize, sr: i32, args: &Args) -> Result<Vec<f64>> {
    let mut nf_db = args.noise_floor; if nf_db > 0.0 { nf_db = -nf_db; warn(...) }
    let level_lin = 10f64.powf(args.level_diff / 20.0);
    let noise_lin = 10f64.powf(nf_db / 20.0);
    if ch > 2 { return Err(TooManyChannels) }
    let bin_frames = (sr * args.bin_ms) / 1000;           // i32
    let bin_samples = bin_frames as usize * ch;
    let frames = inp.len() / ch;
    let n_bins = ((frames as i64 + bin_frames as i64 - 1) / bin_frames as i64) as usize;
    let mut bins = vec![CBin{gain:1.0,peak:0.0,rms:0.0}; n_bins];
    let (mut tot, mut cnt) = (0.0f64, 0u64);
    for b in 0..n_bins {
        let s = &inp[b*bin_samples .. ((b+1)*bin_samples).min(inp.len())];
        if s.is_empty() { return Err(UnexpectedEof) }
        let (mut peak, mut ss) = (0.0f64, 0.0f64);
        for &x in s { let a = x.abs(); if a > peak { peak = a } ss = a*a + ss; }
        bins[b] = CBin{ gain: 1.0, peak, rms: (ss / s.len() as f64).sqrt() };
        if !(noise_lin >= bins[b].rms) { tot = tot + ss; cnt += s.len() as u64; }
    }
    let mean = if cnt == 0 { 0.0 } else { (tot / cnt as f64).sqrt() };
    if mean == 0.0 { return Err(Silent) }
    let thr = mean * level_lin;
    let mut maxp = 0.0f64;
    for b in bins.iter_mut() {
        if b.peak > thr { b.gain = thr / b.peak; let p = b.peak * b.gain; if p > maxp { maxp = p } }
        else if b.peak > maxp { maxp = b.peak }
    }
    let gains = build_gain_curve(&bins, args.attack_ms / args.bin_ms, args.release_ms / args.bin_ms); // see above
    let f = 1.0 / maxp; let gains: Vec<f64> = gains.iter().map(|g| f * g).collect();
    apply_gain_curve(inp, ch, bin_frames, &gains)   // see pass 2
}
```
