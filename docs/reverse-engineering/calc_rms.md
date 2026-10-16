# Step 5 — `calc_rms` (FUN_0003271e)

Source file per assert strings: `/Users/bsharpe/src/level/level/calc_rms/calc_rms.cpp`,
`.../calc_rms/CommandLine.cpp`. Entry FUN_0003271e (asm 0x3271e–0x331f3), arg parser FUN_00033544,
`calc_rms::CCLArgs` ctor 0x33344.

## Invocation by orchestrator (FUN_00028dfc, asm 0x2b44b–0x2b50a)

```
argc = 3
argv = { "calc_rms", <wave in>, <out>_05.txt }
```
- `<wave in>` = the `_04` file (string at ebp-0x174, built at 0x2b02a "_04") normally; if the global
  flag at 0x165aac is non-zero (steps 1–4 skipped, see 0x2a509) it is the string at ebp-0x158 instead
  (the original input).
- No `--bin-size` is passed → default **50 ms** is used.
- Return value != 0 → "Step 5 did not complete successfully".

## Command line (FUN_00033544)

| option | field | type | default |
|---|---|---|---|
| `help` | – | – | – |
| `options-file,o` | CCLArgs+4 (string) | string | – |
| `bin-size,s` "bin size (ms)" | CCLArgs+0 | int | **50** (0x32) |
| hidden positional 1 `input-file` "input audio file" | CCLArgs+8 | string | – |
| hidden positional 2 `output-file` "output text file to hold RMS values" | CCLArgs+0xc | string | – |

The return code of the parser is **ignored** by FUN_0003271e (call at 0x3277a, no test).

## Algorithm (asm 0x327a9–0x330db)

```
info  = SF_INFO zeroed; sf = sf_open(input, SFM_READ /*0x10*/, &info)   // fail -> "Error opening <in>", return 1
F     = info.frames (i64), sr = info.samplerate (i32), ch = info.channels (i32)

// bin length in *samples* (interleaved items), all u32 arithmetic:
bin   = (u32)(bin_ms * ch * sr) / 1000          // imul,imul then *0x10624dd3>>38 == unsigned /1000 (0x3298f..0x329b1)
bin  -= bin % ch                                // force multiple of channels (0x329b8..0x329c4)
total = (i64)F * ch                             // 64-bit (0x329c6..0x329fb)

ofstream out(output, ios::out /*0x10*/)         // fail -> "Error opening <out> for output", return 1
buf   = new f64[bin]                            // "Error in allocating memory for pBinData" on bad_alloc

read_total: u32 = 0                             // NOTE 32-bit counter (ebp-0x4fc)
while (read_total as u64) < total {             // test at 0x330ba (64-bit signed hi / unsigned lo compare)
    n = sf_read_double(sf, buf, bin)            // i64 result, items (samples), interleaved
    read_total += (u32)n
    sum = 0.0
    for i in 0..n { sum = sum + buf[i]*buf[i] }      // sequential f64 accumulation (0x32ef8..0x32f4d)
    if (u32)n != 0 {
        ms   = sum / (f64)(u32)n                     // u32->f64 via (n-2^31)+2147483648.0
        db   = 20.0 * log10(sqrt(ms))                // sqrtsd, libm log10 (x87 result stored to f64), * 20.0 (0x1a6fa0)
        v    = max(-200.0, db)                       // maxsd with -200.0 (0x1a6f98); -inf -> -200
        out << sprintf("%14.12lf\n", v)              // 0x153f4f
    }
    progress(read_total / total)                     // FUN_0005b166, see below
    if n < bin { break }                             // 64-bit compare at 0x3309c: partial (or empty) read ends loop
}
out.close(); return 0
```

### Key facts
- **Units**: dBFS of the RMS (samples as libsndfile doubles, full scale ±1.0): `20*log10(sqrt(mean(x²)))`,
  floored at **-200.0** (digital silence → `-200.000000000000`).
- **Windowing**: plain rectangular, non-overlapping bins of `bin` samples; no window function, no hop.
- **Channel mixing**: none explicitly — the mean square is taken over all interleaved samples of all
  channels in the bin (i.e. average of per-channel mean squares, equal weights). Bin length in frames is
  `bin/ch` = `floor(floor(bin_ms*ch*sr/1000)/ch)` (for 44.1 kHz stereo, 50 ms: 4410 samples = 2205 frames; for
  mono 2205 samples).
- **Final partial bin**: is emitted, averaged over the samples actually read (`sum/n`), then the loop stops.
  If the file length is an exact multiple of `bin`, the loop ends on `read_total == total` and no extra line
  is written. Zero-length reads write nothing. Number of lines = `ceil(total/bin)` (total = F*ch).
- **Number format**: C `"%14.12lf\n"` (12 decimals, min width 14; typical values like `-23.456789012345`
  exceed the width so no padding). One value per line, `\n` line ending.
- Overflow caveat: `bin_ms*ch*sr` is 32-bit unsigned (fine for realistic values).

### Progress (FUN_0005b166)
Progress struct at ebp-0x98: `{start, end, last=-1, cur=-1, cb, ...}`; normal mode start=**34.0**
(0x4041000000000000), end=**35.0** (0x4041800000000000); if flag 0x165aac set start=0.0, end=5.0.
`p = floor(start + (end-start)*frac)`; if `p > last` prints `"\r" << p << "%"` to stdout and flushes
(or invokes callback). If global 0x1a7010 (cancel flag) is set, throws `ss_cancel_exception`.
`frac = (f64)read_total(u32) / (f64)total(u64)`.
If global 0x165aa0 (verbose) set, FUN_00028c7c prints "elapsed time: %4.1f seconds" before the loop.

## Rust-like pseudocode

```rust
fn calc_rms(input: &Path, output: &Path, bin_ms: i32 /*50*/) -> Result<()> {
    let (mut sf, info) = sndfile_open_read(input)?;
    let ch = info.channels as u32;
    let mut bin = (bin_ms as u32).wrapping_mul(ch).wrapping_mul(info.samplerate as u32) / 1000;
    bin -= bin % ch;
    let total: i64 = info.frames * info.channels as i64;
    let mut out = File::create(output)?;
    let mut buf = vec![0f64; bin as usize];
    let mut read_total: u32 = 0;
    while (read_total as i64) < total {
        let n = sf.read_f64_items(&mut buf[..bin as usize]); // sf_read_double
        read_total = read_total.wrapping_add(n as u32);
        let mut sum = 0.0f64;
        for i in 0..n as usize { sum += buf[i] * buf[i]; }
        if n as u32 != 0 {
            let v = 20.0 * (sum / (n as u32 as f64)).sqrt().log10();
            let v = if -200.0 > v { -200.0 } else { v };
            write!(out, "{}", c_fmt("%14.12lf\n", v))?;
        }
        progress(34.0, 35.0, read_total as f64 / total as f64);
        if n < bin as i64 { break; }
    }
    Ok(())
}
```

## Consumers
`_05.txt` is NOT read by calculate_gains. It is the input of step 6 (wrap_segment/segment), which produces
the cue list `_06.clt`.
