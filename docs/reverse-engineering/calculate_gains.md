# Step 7 — `calculate_gains` (FUN_0003646c → FUN_000350b6)

Sources (assert strings): `.../calculate_gains/calculate_gains.cpp`, `.../calculate_gains/CalcRMS.cpp`,
`.../calculate_gains/CommandLine.cpp`.
- FUN_0003646c: wrapper; calls FUN_000350b6, catches `bad_alloc` → "Out of memory." → returns 1.
- FUN_000350b6: main (asm 0x350b6–0x3646a).
- FUN_00036932: argument parser; FUN_0003692c: empty.
- FUN_000343d2: `CalcRMS` — gated RMS of a span of samples (the only one used; 2 call sites 0x359d7, 0x35b60).
- FUN_00034962: an alternative energy-gated RMS (circular buffer of squares); **no direct call sites found**
  — appears unused.

## Invocation by orchestrator (FUN_00028dfc, asm 0x2bb20–0x2bcbe)

```
argc = 13
argv = { "calculate_gains",
         "--noise-threshold", "0.006",
         "--bin-size",        "50",
         "--gains-file",      <out>_08.txt,     // ebp-0x184
         "--rms-file",        <out>_07.txt,     // ebp-0x180
         "--gain-limit",      "24.0",
         <wave in>,                              // positional 1 = wave-file: _04 (ebp-0x174), or ebp-0x158 if flag 0x165aac
         <out>_06.clt }                          // positional 2 = cuelist-file (ebp-0x17c)
```
**Note: the brief has _07/_08 swapped.** `--gains-file` gets `_08.txt` and `--rms-file` gets `_07.txt`
(strings built at 0x2bb20 "_07.txt"→ebp-0x180 and 0x2bb90 "_08.txt"→ebp-0x184; argv slots at 0x2bc3d/0x2bc55).
Step 8 (smooth_gains, 0x2bf77) then reads ebp-0x184 = `_08.txt` = the gains file. Option-value strings
come from static strings: 0x1f1468="0.006", 0x1f1458="50", 0x1f145c="24.0" (static init at 0x583c7..0x5855f).
`_05.txt` (calc_rms output) is **not** used by this step.

## Command line (FUN_00036932) — CCLArgs layout (ebp-0xf4 in main)

| option | CCLArgs offset | type | default |
|---|---|---|---|
| (derived) cue-list-given = `cuelist_file != ""` | +0x00 (u8) | bool | |
| (derived) gain-limit-given = `vm.count("gain-limit")` | +0x01 (u8) | bool | |
| `gain-limit,l` "limit maximum gain to this value (dB)" | +0x04 | f64 | none; if given, replaced by `fabs()` |
| `buffer-length,b` "circular buffer length (s)" | +0x0c | f64 | **0.005** (0x3f747ae147ae147b) |
| `noise-threshold,t` "noise threshold (linear)" | +0x14 | f64 | **0.006** (0x3f789374bc6a7efa) |
| `bin-size,s` "bin size (ms)" | +0x1c | u32 | **50** |
| hidden `cuelist-file` "cue list file (in)" | +0x20 | string | positional 2 |
| hidden `gains-file` "gains file (out)" | +0x24 | string | |
| `options-file,o` | +0x28 | string | |
| hidden `rms-file` "RMS file (out)" | +0x2c | string | |
| hidden `wave-file` "wave file (in)" | +0x30 | string | positional 1 |

Defaults in the binary's own static strings for rms/gains file: "rms.txt", "gains_raw.txt" (not used here,
orchestrator passes both). The orchestrator does not pass `--buffer-length`, so **0.005 s** is used.

## Cue list format (_06.clt, written by wrap_segment at 0x56160..0x5624f)

```
 CueListTool 1.3 file - do not edit unless you know exactly what you do!\n
Prefix=\n
<int>\n                      (an int argument of wrap_segment, likely the sample rate)
then per breakpoint:
<cue>\n0\n\n\n\n             (cue = bp * (int)(<int> * <double>), i.e. a position in sample FRAMES)
```
calculate_gains skips 3 lines, then repeatedly does `>> u64` and 4× `getline(buf,256)` (the 1st getline eats
the rest of the number line, then "0", "", ""). It interprets cue values as **frame positions**.

## Algorithm

### Setup (0x3514c–0x3542f)
```
sf = sf_open(wave, SFM_READ, &info)        // fail -> "Error opening <wave>", return 1
F  = info.frames (u64), sr = info.samplerate, ch = info.channels
total_samples = F * ch                     // 64-bit (ebp-0xaa8/0xaa4), only used in no-cue branch
buf_frames    = sat_u32_trunc((f64)sr * buffer_length + 1.0)   // 0x353bb..0x353f8; 44100 -> 221, 48000 -> 241
buf_len       = ch * buf_frames            // u32, ring length in samples (ebp-0xab8)
bin_frames    = (u32)(sr * bin_size) / 1000   // u32 mul, unsigned /1000 (magic 0x10624dd3 >>6) (ebp-0xa7c); 44100 -> 2205
rms_out = fstream(rms_file, ios::out /*0x10*/)  // fail -> "error opening RMS file <rms>", return 1
progress range: start=69.0 (0x4051400000000000), end=82.0 (0x4054800000000000)
                (flag 0x165aac: 58.0 / 73.0)
```
`sat_u32_trunc(d)`: `d >= 2^32-1 → 4294967295; d <= 0 → 0; else trunc(d)` (SSE idiom with 2147483648.0 bias).

If no cue list (`cuelist_file == ""`, 0x35b0c): CalcRMS over the whole file is computed and discarded; nothing
is written; return 0. (Never happens in the pipeline.)

### Pass 1 — per-segment level → RMS file (0x35656–0x35b07)
```
cue_in = ifstream(cuelist, ios::in)   // fail -> "error opening cue list file <f>", return 1
skip 3 lines (getline 256)
prev: u64 = 0; done = false; sum_db = 0.0; count: u32 = 0
while !cue_in.fail() && !done {
    if cue_in >> c (u64) succeeded {
        k   = sat_u32_trunc((f64)c / (f64)bin_frames + 0.5)   // round to nearest bin (u64->f64 exact idiom)
        cue = (u64)bin_frames * (u64)k                        // 32x32->64 mul
    } else {
        cue = F; done = true                                   // final segment ends at true file end (unrounded)
    }
    len_samples = (i64)ch * (cue - prev)          // 64-bit (sign-extended ch), may be 0
    rms = CalcRMS(sf, len_samples, buf_len, noise_threshold)   // reads sequentially from current file position
    if rms > 0.0 { v = 20.0*log10(rms); count += 1; sum_db += v }   // log10 x87 -> f64, then *20.0
    else         { v = -200.0 }
    rms_out << snprintf("%5.2lf", v) << endl      // one line per segment, incl. -200.00 lines
    skip 4 lines (getline 256)
    prev = cue
}
mean_db = sum_db / (f64)count          // u32->f64; count==0 -> NaN (0/0)
rms_out.close()
```
Segments are therefore `[0,c0) [c0,c1) ... [c_{n-1}, F)` (cues rounded to bin multiples, last one ends at F);
**RMS file has (#cues + 1) lines** (if the first cue is 0 the first segment is empty and prints `-200.00`).
Since the sndfile is never seeked, each CalcRMS continues where the previous stopped; with len 0 nothing is read.

### CalcRMS (FUN_000343d2): gated RMS with sliding-peak gate
Args: `(SNDFILE*, n_items: i64, L: u32 /*ring len, samples*/, thr: f64)`
```
ring = [0.0; L]; wp = 0; peak = 0.0; sum = 0.0; cnt: u64 = 0
chunk = new f64[0x40000]                  // 0x200000 bytes
remaining = n_items
got = sf_read_double(sf, chunk, min(remaining, 0x40000))   // if remaining <= 0x3ffff (signed) use remaining
while got > 0 {                           // signed 64-bit (0x34875)
    for i in 0..got {
        a = |chunk[i]|                    // andpd with abs mask
        let rescan;
        if a > peak { peak = a; rescan = false }
        else        { rescan = !(peak > ring[wp]) }   // i.e. peak <= outgoing value (setbe)
        ring[wp] = a; wp += 1
        if rescan { peak = 0.0; for k in 0..L { peak = if ring[k] > peak { ring[k] } else { peak } } }
        if peak > thr { cnt += 1; sum = a*a + sum }     // strict >
        if wp > L-1 { wp = 0 }
    }
    remaining -= got
    got = sf_read_double(sf, chunk, min(remaining, 0x40000))
}
return if cnt == 0 { 0.0 } else { sqrt(sum / (f64)cnt) }     // u64->f64 exact
```
So: a sample is included in the segment RMS iff the max |x| over the last `L` interleaved samples
(current one included; ring pre-filled with zeros at each segment start) is **> noise_threshold (0.006 linear,
≈ -44.4 dBFS)**. `peak` is exactly the trailing-window max (the literal algorithm is reproduced above).
Channels: interleaved samples treated as one stream (L = ch*buf_frames; for stereo 44.1k L = 442).
Error string "Error in allocating memory for pBufStart" (line 0x2b) on bad_alloc.

### Pass 2 — gains file (0x35b9c–0x36356)
```
rms_in  = fstream(rms_file, ios::in /*8*/)              // fail -> "error opening RMS file"
gains   = ofstream(gains_file, ios::out|ios::trunc /*0x30*/)  // fail -> "error opening gains file"
cue_in.clear(); cue_in.seekg(0, beg); skip 3 lines
prev = 0
for i in 0..count {                                     // NOTE: count = #segments with rms>0, not #lines
    rms_in >> x (f64)                                   // fail -> "error reading from <rms>", return 1
    x = x - mean_db                                     // x is the %5.2lf-rounded value read back
    if gain_limit_given { x = tanh(x / L) * L }         // L = fabs(gain_limit) = 24.0
    g = 1.0 / pow(10.0, x / 20.0)                       // linear gain = 10^(-x/20)
    if i == count - 1 {
        cue = F + 1                                     // unrounded, frames
    } else {
        cue_in >> c; skip 4 lines
        cue = bin_frames * sat_u32_trunc((f64)c/(f64)bin_frames + 0.5)
    }
    n = ((cue - prev + bin_frames - 1) / bin_frames) as u32   // u64 ceil-div (___udivdi3), truncated to u32
    repeat n times: gains << g << endl                  // ostream<<double default: precision 6, %g style
    if count > 1 { progress((f64)i / (f64)count) }
    prev = cue
}
progress(1.0); return 0
```
- **Gains file** (`_08.txt`): one linear gain per bin (bin = `bin_frames` frames = 50 ms), i.e. each segment's gain
  repeated `ceil(segment_len/bin_frames)` times; formatted by `std::ostream << double` with default flags
  (6 significant digits, e.g. `1.58489`, `0.891251`, `15.8489`, scientific for very small/large), `\n` per line.
  Total lines normally = `floor(F/bin_frames) + 1` (last segment counted up to F+1).
- **RMS file** (`_07.txt`): one line per segment, `"%5.2lf"` of the gated segment level in dBFS
  (`20*log10(rms)`) or `-200.00` for an empty/all-below-threshold segment.
- **Target level**: the mean (arithmetic mean in dB) of all segment levels with rms > 0. Each segment is
  pushed toward that mean: gain_dB = -(level − mean); with gain-limit the deviation is soft-limited by
  `L*tanh(dev/L)`, so |gain| < 24 dB asymptotically (both boost and cut).
- No weighting by segment length in the mean.

### Literal quirks to reproduce
1. Pass 2 iterates `count` times (segments with rms>0), but reads the RMS file line by line. If any segment
   printed `-200.00` (empty or fully gated), lines and cue positions misalign: that `-200.00` is used as a level
   (→ deviation strongly negative → gain ≈ +24 dB after tanh limit, `10^(L·tanh((−200−mean)/L)/20)` ≈ 15.85), and
   the last `#lines − count` segments are never emitted separately — the iteration `i == count−1` uses `F+1` as
   the end, so the final gain value spans to the end of file. A zero-length first segment (cue 0) gives n = 0
   lines, so its only effect is shifting the final-segment handling.
2. The deviation uses the 2-decimal value read back from the text file, while `mean_db` is computed from
   full-precision values.
3. If `count == 0`: mean is NaN, the gains file is empty.
4. Rounded cues could exceed F or be non-monotonic; then `cue - prev` wraps (u64) / is negative (i64) — no guards.
5. `getline` into a 256-byte buffer: lines ≥ 255 chars would set failbit (not an issue with CLT files).
6. libsndfile requires item counts divisible by `ch`; chunk 0x40000 is fine for 1/2/4/8 channels only.

## Rust-like pseudocode (core)

```rust
fn calculate_gains(a: &Args) -> Result<()> {
    let (mut sf, info) = sndfile_open_read(&a.wave_file)?;
    let (f, sr, ch) = (info.frames as u64, info.samplerate as u32, info.channels as u32);
    let buf_frames = sat_u32(sr as f64 * a.buffer_length + 1.0);   // trunc
    let ring_len = ch.wrapping_mul(buf_frames);
    let bin = sr.wrapping_mul(a.bin_size) / 1000;
    let round_cue = |c: u64| bin as u64 * sat_u32(c as f64 / bin as f64 + 0.5) as u64;

    // pass 1
    let cues = read_clt_cues(&a.cuelist_file)?;         // all successfully parsed u64s, in order
    let mut rms_txt = String::new();
    let (mut prev, mut sum_db, mut count) = (0u64, 0.0f64, 0u32);
    for end in cues.iter().map(|&c| round_cue(c)).chain(std::iter::once(f)) {
        let len = (ch as i64).wrapping_mul(end.wrapping_sub(prev) as i64);
        let r = calc_rms_gated(&mut sf, len, ring_len, a.noise_threshold);
        let v = if r > 0.0 { let v = 20.0 * r.log10(); count += 1; sum_db += v; v } else { -200.0 };
        rms_txt += &format!("{}\n", c_fmt("%5.2lf", v));
        prev = end;
    }
    write(&a.rms_file, &rms_txt)?;
    let mean = sum_db / count as f64;

    // pass 2
    let levels: Vec<f64> = parse_ws_f64(&rms_txt);   // re-read from file text
    let mut out = String::new(); prev = 0;
    for i in 0..count as usize {
        let mut x = levels[i] - mean;
        if a.gain_limit_given { let l = a.gain_limit.abs(); x = (x / l).tanh() * l; }
        let g = 1.0 / 10f64.powf(x / 20.0);
        let end = if i == count as usize - 1 { f + 1 } else { round_cue(cues[i]) };
        let n = (end.wrapping_sub(prev).wrapping_add(bin as u64 - 1) / bin as u64) as u32;
        for _ in 0..n { out += &format!("{}\n", cpp_ostream_double(g)); } // %g, precision 6
        prev = end;
    }
    write(&a.gains_file, &out)
}
```
