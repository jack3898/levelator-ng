# Step 4 — `reduce_loud` (entry FUN_0004533c)

Status: fully traced from asm. The algorithm is structurally identical to `reduce_loud_peak`
(see specs/reduce_loud_peak.md); the differences are (a) the per-bin loudness compared with the threshold
is the bin **RMS** (not peak), (b) the bin RMS for stereo is the **max of the per-channel RMS**,
(c) defaults/pipeline values (level_diff 3 dB, release 150 ms), (d) no verbosity option, warning goes through a
logger with `endl`, (e) progress ranges 27→30 / 30→34.

## Call chain

| addr | role |
|---|---|
| 0x4533c → 0x452e6 | sub-program `main(argc, argv)`; returns 0 |
| 0x4439e → 0x432ba | build `CCLArgs` (0x44632 → defaults 0x453ea) and parse argv (reduce_loud/CommandLine.cpp) |
| 0x46d2c | `ReduceLoudOperation op(args); op.run();` |
| 0x457fc | ctor: noise-floor sign fix + dB→linear |
| 0x466be | `run()` |
| 0x45ff4 | pass 1: per-bin peak / RMS (per channel) + mean RMS |
| 0x454f8 | per-bin gain = thr/rms for loud bins; returns max post-gain peak |
| 0x4561a (+0x454d4 fill) | gain curve with attack/release ramps |
| 0x4546a | scale gains by 1/maxPeak |
| 0x45a7c | pass 2: apply interpolated gain, write |
| 0x45290 | 64-bit ceil-div |
| 0x43214 | "Noise floor / Level difference" snprintf only under global verbose flag; never output. No effect. |

0x43214 and 0x432ba belong to **reduce_loud** (they reference reduce_loud/CommandLine.cpp and are reached from
0x4439e), not to normalize.

## Arguments (`ReduceLoudArgs`, ctor 0x453ea; option binding 0x43387..0x4354d)

| offset | type | option | default | pipeline |
|---|---|---|---|---|
| 0x00 | f64 | `--noise-floor,-f` | -44.0 | "-44.0" |
| 0x08 | f64 | `--level_diff,-l` "maximum level above mean (dB)" | 3.0 (0x40080000) | "3.0" |
| 0x10 | i32 | `--attack,-a` | 0 | "0" |
| 0x14 | i32 | `--release,-r` | 150 (0x96) | "150" |
| 0x18 | i32 | `--bin-size,-s` | 50 | "50" |
| 0x1c / 0x20 | string | positional in / out | | `<out>_03` / `<out>_04` |

No verbosity option. Orchestrator (0x2b0e3..0x2b19a) calls `FUN_0004533c(13, argv)` with
`reduce_loud --level_diff 3.0 --attack 0 --release 150 --noise-floor -44.0 --bin-size 50 <_03> <_04>`
(static strings 0x1f141c="3.0", 0x1f1418="0", 0x1f1414="150", 0x1f1410="-44.0", 0x1f1458="50").

## Operation layout

0x24 noiseLin, 0x2c levelLin, 0x34 threshold, 0x3c maxPeak, 0x44 meanRms, 0x4c SNDFILE*, 0x54 binFrames,
0x58 binSamples, 0x5c nBins, 0x60 nBins+1, 0x64 CBin[nBins], 0x68 gains[nBins+1], 0x6c SF_INFO
(0x6c frames, 0x74 samplerate, 0x78 channels). CBin = {+0 gain, +8 peak, +0x10 rms}.

## Constructor (0x457fc)

```
if noise_floor_db > 0.0 {
    noise_floor_db = -noise_floor_db;
    // snprintf("Warning: Specified noise floor was positive.  Value has been changed to %.2lf.", nf)
    // → ostringstream → FUN_0005add6 → std::cerr << msg << endl
}
levelLin = pow(10, level_diff/20)   // 10^(0.15) ≈ 1.4125375446227544
noiseLin = pow(10, noise_floor/20)  // 10^(-2.2) ≈ 0.006309573444801929
```

## run() (0x466be)

Identical to reduce_loud_peak run():
`sf_open(in, SFM_READ)` (fail → ss_sf_openinput_error); `channels > 2` → ss_sf_too_many_channels_error;
`binFrames = samplerate*bin_ms/1000` (i32 trunc); `binSamples = binFrames*channels`;
`nBins = ceil(frames/binFrames)` (i64); `meanRms = measure()`; `sf_seek(0)`;
`meanRms == 0.0` → ss_sf_input_silent_error (" is silent");
`threshold = meanRms * levelLin`; compute gains; curve(1.0); scale(1.0); write.

## Pass 1: measure (0x45ff4) — per-channel RMS

```
for b in 0..nBins {
    n = sf_read_double(file, buf, binSamples)          // items
    if n == 0 { throw ss_sf_unexpected_eof(in) }
    peak = 0.0; s0 = 0.0; s1 = 0.0
    k = 0
    while k < n {
        a = |buf[k]|; if a > peak { peak = a }; s0 = a*a + s0
        if ch == 2 {
            k += 1
            a = |buf[k]|; if a > peak { peak = a }; s1 = a*a + s1
        }
        k += 1
    }
    fr = (n / ch) as f64                                // i32 division
    rms = if ch == 1 { sqrt(s0 / fr) }
          else /*ch==2*/ { let l = sqrt(s0/fr); let r = sqrt(s1/fr); if r > l { r } else { l } }
    bin[b] = { gain: 1.0, peak, rms }                  // peak = max |x| over BOTH channels
    if !(noiseLin >= rms) {                            // rms > noiseLin
        if ch == 1 { tot = tot + s0 }
        else       { tot = tot + (s0 + s1) }          // exact order: tmp = s0 + s1; tot = tot + tmp
        cnt += n                                       // u64, counts all interleaved samples
    }
    progress(27.0 → 30.0, b/(nBins-1) or 1.0)
}
meanRms = if cnt == 0 { 0.0 } else { sqrt(tot / cnt as f64) }   // combined RMS over all samples of non-quiet bins
```
Note the asymmetry: the per-bin value uses the louder channel's RMS, the mean uses the combined (power-average)
RMS over all samples. The noise-floor test uses the max-channel bin RMS.

## Per-bin gain (0x454f8)

```
maxPeak = 0.0
for i in 0..nBins {
    if bin[i].rms > threshold {                         // jbe → else
        bin[i].gain = threshold / bin[i].rms
        p = bin[i].peak * bin[i].gain
        if p > maxPeak { maxPeak = p }
    } else if bin[i].peak > maxPeak { maxPeak = bin[i].peak }
}
```

## Gain curve (0x4561a) — byte-for-byte same algorithm as reduce_loud_peak 0x494f0

```
gains.fill(1.0)    // nBins+1 boundary gains
for i in 0..nBins {
    att = attack_ms / bin_ms; rel = release_ms / bin_ms     // i32 trunc → pipeline: att = 0, rel = 150/50 = 3
    g = bin[i].gain
    if g < 1.0 {
        stepR = (1.0 - g) / (rel + 1) as f64
        cur = g; for j in (i+1)..min(rel + i + 2, nBins + 1) { gains[j] = min(cur, gains[j]); cur = stepR + cur }
        stepA = (1.0 - g) / (att + 1) as f64
        cur = g; j = i; while j > max(i - att - 1, -1) { gains[j] = min(cur, gains[j]); cur = stepA + cur; j -= 1 }
    }
}
```
With pipeline values: a loud bin i with gain g sets boundary i and i+1 to g, then release ramps
boundaries i+2, i+3, i+4 to g + k·(1-g)/4 for k = 1,2,3 (taking the min with what is already there);
boundary i+5 is untouched (back to 1.0 unless another bin lowers it). No attack look-ahead (att = 0).
In time: gain is constant over the loud bin, then rises linearly back towards 1 over 4 bins (200 ms) of
per-sample linear interpolation, reaching 1.0 at boundary i+5.
`min` = SSE `minsd` (`if cur < x {cur} else {x}`).

## Normalisation (0x4546a)

`f = 1.0 / maxPeak; for j in 0..=nBins { gains[j] = f * gains[j] }` — peak-normalises to 1.0 again.

## Pass 2: write (0x45a7c) — same as reduce_loud_peak 0x49806

`sf_open(out, SFM_WRITE=0x20, &sfinfo_of_input)`; per bin: `sf_readf_double(binFrames)`,
`g = gains[b]`, `step = (gains[b+1]-gains[b]) / binFrames as f64`, per frame multiply all channels by g then
`g = g + step`; `sf_writef_double`, short write → ss_sf_io_error. Progress 30.0 → 34.0.
No sf_command calls; libsndfile default double conversions.

## Rust-like core (differences from reduce_loud_peak only)

```rust
let (rms, sum_for_mean) = if ch == 1 {
    ((s0 / fr).sqrt(), s0)
} else {
    let l = (s0 / fr).sqrt(); let r = (s1 / fr).sqrt();
    (if r > l { r } else { l }, s0 + s1)
};
// loud test & gain use rms instead of peak:
if b.rms > thr { b.gain = thr / b.rms; let p = b.peak * b.gain; if p > maxp { maxp = p } }
else if b.peak > maxp { maxp = b.peak }
```
