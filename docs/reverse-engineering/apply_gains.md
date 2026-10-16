# Step 9 — apply gains (FUN_0002f9b8)

Sources: `/Users/bsharpe/src/level/level/apply_gains/common.h` (InputSoundFile / OutputSoundFile / FrameArray helpers).
Evidence: re/fn/02f9b8.c, asm 0x2f9b8–0x300b0 (dumped to work/ag.asm), helpers 0x30802 (read block),
0x31386 (write block), 0x30b46/0x30c84 (open input), 0x31186/0x31302/0x3137c (open output), 0x307d8 (seek).

## Parameters — passed through globals (orchestrator asm 0x2c2ac–0x2c318)

| global (ptr slot → storage) | set to | meaning |
|---|---|---|
| 0x1f33e0 → **0x1f13a8** (int) | `0x32` = **50** | bin size in ms |
| 0x1f3408 → **0x1f13a4** (std::string) | `<base>_09.txt` | smoothed gains file (smooth_gains output) |
| 0x1f341c → **0x1f13a0** (std::string) | `<base>_04<ext>` (reduce_loud output; the original input if the compile-time "skip preprocessing" flag 0x165aac were 1 — it is 0) | input audio |
| 0x1f33a0 → **0x1f139c** (std::string) | `<base>_10<ext>` | output audio |

The function takes no arguments and **returns void**; the orchestrator ignores any result and instead checks that
`<base>_10<ext>` exists (`"Could not find <path>"` on stderr → return 1). Errors inside are C++ exceptions
(`sf_open_input_err` "Error opening X for input", `sf_open_output_err`, `sf_read_err` "Error reading from X",
`sf_write_err`, `incompatible_frames_err`) which propagate out of the orchestrator.

## File handling
- Gains: `ifstream(0x1f13a4)`, `copy(istream_iterator<double>, eof, back_inserter(g))` — whitespace-separated
  doubles parsed with `operator>>`. Then **`g.push_back(0.0)`** (asm 0x2fa..: local 0.0 pushed). So `len = n+1`.
- Input: `sf_open(path, SFM_READ=0x10, &info)` (info zeroed first). Read with `sf_readf_double`
  (libsndfile normalisation ON, default: PCM16 → x/32768 etc.).
- Output: `sf_open(path, SFM_WRITE=0x20, {format=in.format, channels=in.channels, samplerate=in.samplerate})`
  → **same container/subtype/rate/channels as the input** (FUN_00031302 copies fields +0x1c/+0x18/+0x14). Written with
  `sf_writef_double` (normalisation ON, **no clipping** command, no dither). libsndfile shipped = **1.0.21**
  (PCM16 write = `lrint(x * 32767.0)` stored to short, wraps on overflow; read = `s / 32768.0`).
- Channels: any count; every channel is multiplied by the same gain (no per-channel processing).

## Bin geometry
```
bin_frames = (samplerate * 50) / 1000            // int32 signed division (imul + magic, asm 0x2fb19–0x2fb3e)
```
Gains index i covers frames [i*bin_frames, (i+1)*bin_frames). Gain values are **linear** (from calculate_gains /
smooth_gains) — no dB conversion here.

## Pass 1 — peak of the gained signal (asm 0x2fb49–0x2fc79)

```
peak = 0.0
for i in 0 .. len-1:                        // i < g.len()-1  (== original n)
    blk = readf(bin_frames)                 // may return fewer (or 0) frames at EOF; no early exit
    a  = g[i]
    d  = (g[i+1] - g[i]) / (bin_frames as f64)   // computed ONCE per bin (cvtsi2sd + divsd at 0x2fbc4)
    for f in 0..blk.frames:
        for c in 0..channels:
            v = abs(a * blk[f][c])          // mulsd then andpd(abs)
            peak = if v > peak { v } else { peak }   // maxsd v, peak
        a = a + d                           // addsd after each frame
```
Note the final bin interpolates from g[n-1] toward the appended **0.0** (a linear fade-out over the last 50 ms bin).

Then `sf_seek(in, 0, SEEK_SET)` (0x307d8).

## Pass 2 — apply and write (asm 0x2fd7e–0x2ffd6)

```
for i in 0 .. len-1:
    blk = readf(bin_frames)
    if blk.frames == 0 { break }            // EOF → stop
    a = g[i] / peak                         // divsd
    d = (g[i+1] / peak - a) / (bin_frames as f64)   // (g1/peak - a) then / bin_frames, once per bin
    for f in 0..blk.frames:
        for c in 0..channels:
            blk[f][c] = a * blk[f][c]       // mulsd a, x
        a = a + d
    writef(out, blk)                        // throws if written != blk.frames
    progress(frac)                          // see below
```
Consequences:
- The output is normalised so that its **absolute peak is exactly 1.0** (in double, before PCM quantisation).
- Frames beyond `n*bin_frames` (more audio than gain values) are **not written** (output truncated).
- If fewer frames than n bins exist, output length = input length.
- peak == 0 → division by zero (inf/NaN gains); not guarded.
- Empty gains file: len = 1 → no bins → output file has a header and 0 frames.

## Progress (FUN_0005b166, see orchestrator.md)
Struct `{start, end, last=-1, cur=-1, cb=*0x1f1398, ud=*0x1f1394}`; start/end = **82.0 → 91.0**
(0x40548000…, 0x4056c000…; the 74→83 alternative is only for the disabled skip-preprocessing mode).
`frac = if len < 3 { 1.0 } else { i as f64 / (len - 2) as f64 }` (unsigned→double, i is the bin just written),
printed value = `floor(82 + 9*frac)`.

## Rust reference

```rust
pub fn apply_gains(inp: &mut SndIn, out: &mut SndOut, gains_smoothed: &[f64]) {
    let mut g = gains_smoothed.to_vec(); g.push(0.0);
    let bf = (inp.samplerate * 50) / 1000;            // i32
    let ch = inp.channels as usize;
    let nb = g.len() - 1;
    let mut peak = 0.0f64;
    for i in 0..nb {
        let blk = inp.readf(bf);                       // Vec<f64> interleaved, normalised
        let mut a = g[i]; let d = (g[i+1] - g[i]) / bf as f64;
        for fr in blk.chunks(ch) { for &x in fr { let v = (a*x).abs(); if v > peak { peak = v; } } a += d; }
    }
    inp.seek(0);
    for i in 0..nb {
        let mut blk = inp.readf(bf); if blk.is_empty() { break; }
        let mut a = g[i] / peak; let d = (g[i+1] / peak - a) / bf as f64;
        for fr in blk.chunks_mut(ch) { for x in fr.iter_mut() { *x = a * *x; } a += d; }
        out.writef(&blk);
        let frac = if g.len() < 3 { 1.0 } else { i as f64 / (g.len() - 2) as f64 };
        progress(82.0, 91.0, frac);
    }
}
```
