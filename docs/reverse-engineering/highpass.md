# Step 1 — `highpass` (FUN_0003e48c → FUN_0003d836)

Status: implementation-ready. All constants were read from `bin/level.ann.asm`. The libm stubs were resolved
with `llvm-objdump --indirect-symbols`: 0x1f7959=_sin, 0x1f785a=_cos, 0x1f795e=_sinh, 0x1f78fa=_pow,
0x1f78b9=_log, 0x1f78c3=_log10, 0x1f796d=_sprintf, 0x1f792c=_sf_read_double, 0x1f794f=_sf_write_double,
0x1f7945=_sf_seek, 0x1f7927=_sf_open.

## 1. What the orchestrator passes (FUN_00028dfc @ 0x2a5ca..0x2a653)

```
argv = ["highpass",
        "--highpass__low",       *0x1f1454  = "25.0",
        "--highpass__high",      *0x1f1450  = "100.0",
        "--highpass__verbosity", *0x1f144c  = "0",
        <in>  (orchestrator local ebp-0x158),
        <out> (ebp-0x190 = <out>_01)]        argc = 9
```
`FUN_0003e48c` returns a **double** (x87 st0), which is stored at `[ebp-0xfd0]`:
* `if !(L < 0.0)`, the orchestrator fails with "Step 1 did not complete successfully" (`ucomisd L, 0.0 ; jb ok`).
  This covers L == 0.0, which is what highpass returns for silent input. It does not cover NaN, because NaN
  sets CF and so takes the "ok" branch.
* Otherwise, at 0x2a92a: `thr = minsd(-40.0, L - 18.0)` (constants: f64 18.0 @0x1a6f78, f64 -40.0 @0x1a6f80),
  so `thr = if -40.0 < L-18.0 { -40.0 } else { L-18.0 }`. It then does `sprintf(buf, "%.1lf", thr)`, and
  `buf` is passed as **both** `--gate__threshold_attack` and `--gate__threshold_release` (see gate.md).
  This is the "%.1lf" value: it is a gate threshold derived from the data.

## 2. Arguments (HighpassArgs, ctor 0x3e82e, parser FUN_0003ca36)

| offset | field | option | built-in default |
|---|---|---|---|
| +0x00 f64 | low  | `highpass__low,l`  "low frequency cutoff (Hz)"  | 25.0 (0x40390000) |
| +0x08 f64 | high | `highpass__high,g` "high frequency cutoff (Hz)" | 100.0 (0x40590000) |
| +0x10 f64 | center (derived) | | |
| +0x18 f64 | bandwidth_octaves (derived) | | |
| +0x20 str | sound-file-in (positional 1) | | |
| +0x24 str | sound-file-out (positional 2) | | |
| +0x28 str | options-file (`-o`) | | |
| +0x2c i32 | verbosity (`highpass__verbosity,v`) | | 0 |

Derived values (FUN_0003c830):
```
center = (low + high) * 0.5                 // 62.5
bw     = ln(high / low) / ln(2.0)           // 2.0 (log(4)/log(2) is exactly 2.0 in f64)
```
FUN_0003c8b8 only prints these values when verbosity > 0.

The "low" and "high" values are therefore **not** two separate cutoffs. The program builds one 2nd-order RBJ
high-pass biquad whose f0 is the arithmetic mean (62.5 Hz) and whose bandwidth parameter is the octave span
between them (2 octaves).

## 3. Filter design: FUN_0003c3da(type, dBgain, f0, fs, bw, &biquad) (Robert Bristow-Johnson cookbook)

The call site (0x3daef..0x3db08) passes `type = 1` (HPF), `dBgain = 0.0`, `f0 = center`,
`fs = (double)SF_INFO.samplerate` (cvtsi2sd of the int at 0x1f13d0), and `bw = bandwidth`.
The exact operation order from the asm is:
```
A     = pow(10.0, dBgain / 40.0)          // 1.0; not used by the HPF case
w0    = (f0 * 6.283185307179586) / fs     // f64 @0x1a6fb8
s     = sin(w0)
c     = cos(w0)
arg   = (w0 * (bw * 0.34657359027997264)) / s   // f64 @0x1a6fc0 = ln(2)/2
alpha = sinh(arg) * s
// case 1 (HPF), 0x3c52a:
b0 = (c + 1.0) * 0.5
b1 = -(c + 1.0)          // sign flip by xorpd with -0.0
b2 = b0
a0 = alpha + 1.0
a1 = c * -2.0
a2 = 1.0 - alpha
coef = [b0/a0, b1/a0, b2/a0, a1/a0, a2/a0]   // each is an individual divsd
state x1 = x2 = y1 = y2 = 0.0                // zeroed at 0x3c7bd..0x3c809
returns true (types 0..6 are LPF, HPF, BPF, notch, peak, lowshelf, highshelf; >6 returns false)
```
Biquad struct layout (0x48 bytes): `[b0,b1,b2,a1,a2,x1,x2,y1,y2]`.
At fs = 44100 the result is a 2nd-order Butterworth-like HPF with Q = 1/(2·sinh(ln2/2·bw·w0/sin w0)) ≈ 0.667 and f0 = 62.5 Hz.

## 4. Per-sample filter: FUN_0003c364(x, &bq), Direct Form I

Exact operation order (asm 0x3c364):
```
y = (((x*b0 + x1*b1) + b2*x2) - a1*y1) - a2*y2
x2 = x1; x1 = x;  y2 = y1; y1 = y
```
Multiplication is commutative and every term is rounded to f64 before it is added, so this order gives
bit-exact results.

## 5. Main loop: FUN_0003d836

```
in  = SoundFile::open_read(path_in, normalize=true)  // FUN_0000408e→3f32→3d54: sf_open(SFM_READ=0x10), sf_command(SFC_SET_NORM_DOUBLE=0x1012, true)
info = in.SF_INFO  → copied to global 0x1f13c8 {frames:i64 @13c8, samplerate @13d0, channels @13d4, format @13d8, ...}
if channels > 2 → throw highpass::ss_sf_too_many_channels_error
out = SoundFile::create_like(path_out, in)          // FUN_00003d4a→3bfa→3916: sf_open(SFM_WRITE=0x20, &copy_of_in.SF_INFO), same norm flag
                                                     // → output has the SAME format/subtype/samplerate/channels as input
design(&bqL); if channels == 2 { design(&bqR) }      // identical coefficients, independent zeroed state
block = 8192 - 8192 % channels                        // items (samples), 8192 for mono and stereo
sumL = 0.0; sumR = 0.0
n = in.read_double(buf, block)                        // FUN_000036a4: sf_read_double(min(block, remaining_frames*channels))
while n > 0 {
    i = 0
    loop {                                            // do-while: body runs at least once
        y = biquad(buf[i], &bqL); y = clamp(y);  buf[i] = y; sumL = y*y + sumL
        if channels == 2 { i += 1; y = biquad(buf[i], &bqR); y = clamp(y); buf[i] = y; sumR = y*y + sumR }
        i += 1
        if !(i < n) break
    }
    w = n - n % channels
    out.write_double(buf, w)                          // FUN_00003832: sf_write_double, throw runtime_error if short write
    progress(...)                                     // no effect on output
    n = in.read_double(buf, block)
}
m = if sumR <= sumL { sumL } else { sumR }            // asm 0x3e342: ucomisd R,L ; cmovbe → pick L when R<=L
if m == 0.0 { return 0.0 }
return 20.0 * log10( sqrt( m / (frames as f64) ) )    // frames = SF_INFO.frames from the input; f64 20.0 @0x1a6fa0
```
`clamp(y)` (asm 0x3dd99..0x3ddba: cmpnltsd/maxsd/andpd/andnpd/orpd):
```
t = if -1.0 > y { -1.0 } else { y }   // maxsd(-1.0, y); a NaN y passes through as y
r = if !(1.0 < y) { t } else { 1.0 }  // so the result is min(1, max(-1, y)), and NaN stays NaN
```
The clamped value is both written to the file and accumulated into the energy sum.

The return value is the dB RMS level of the **filtered** signal (the louder channel by total energy, and not
the mean of the channels): `20·log10(sqrt(Σy²/frames))`. It is used only by the orchestrator to build the gate threshold (§1).

## 6. Rust-like reference

```rust
struct Biquad { b0: f64, b1: f64, b2: f64, a1: f64, a2: f64, x1: f64, x2: f64, y1: f64, y2: f64 }
fn design_hpf(f0: f64, fs: f64, bw: f64) -> Biquad {
    let w0 = (f0 * 6.283185307179586) / fs;
    let s = w0.sin(); let c = w0.cos();
    let alpha = ((w0 * (bw * 0.34657359027997264)) / s).sinh() * s;
    let b0 = (c + 1.0) * 0.5; let b1 = -(c + 1.0); let a0 = alpha + 1.0;
    let a1 = c * -2.0; let a2 = 1.0 - alpha;
    Biquad { b0: b0/a0, b1: b1/a0, b2: b0/a0, a1: a1/a0, a2: a2/a0, x1:0.,x2:0.,y1:0.,y2:0. }
}
impl Biquad { fn tick(&mut self, x: f64) -> f64 {
    let y = (((x*self.b0 + self.x1*self.b1) + self.b2*self.x2) - self.a1*self.y1) - self.a2*self.y2;
    self.x2 = self.x1; self.x1 = x; self.y2 = self.y1; self.y1 = y; y } }
fn clamp1(y: f64) -> f64 { let t = if -1.0 > y { -1.0 } else { y }; if !(1.0 < y) { t } else { 1.0 } }
// center = (low+high)*0.5; bw = (high/low).ln() / 2f64.ln();
// process interleaved blocks of 8192 items; ch0 → bqL, ch1 → bqR; accumulate sum = y*y + sum per channel
// level = if m == 0.0 { 0.0 } else { 20.0 * (m / frames as f64).sqrt().log10() }
```
Platform note: macOS libm `sin`, `cos`, `sinh` and `pow` may differ from Rust/glibc by 1 ulp. This only
perturbs the coefficients at the 1e-16 relative level.

## 7. Shared SoundFile helpers (libsndfile 1.0.21 bundled; `strings libsndfile.1.dylib` → "libsndfile-1.0.21")

SoundFile object layout: `+0 SNDFILE* (boost::shared_ptr, 8 bytes)`, `+8 i64 start_frame (0)`,
`+0x18 str`, `+0x24 bool normalize`, `+0x28 i64 end_frame (= SF_INFO.frames)`, `+0x30 str path`,
`+0x34 SF_INFO {i64 frames, i32 samplerate @0x3c, i32 channels @0x40, i32 format @0x44, sections, seekable}`.

| fn | meaning |
|---|---|
| 0x408e / 0x3f32 | `SoundFile(path, normalize)`: open for read (0x3d54: `sf_open(path, SFM_READ, &info)`; throws "Error opening sound file: "+sf_strerror), then `sf_command(SFC_SET_NORM_DOUBLE(0x1012), normalize)`; the range is set to [0, frames) |
| 0x3d4a / 0x3bfa | `SoundFile(path, const SoundFile& like)`: copies `like`'s SF_INFO and normalize flag, then `sf_open(path, SFM_WRITE, &info)` (0x3916). **The output format is identical to the input (container, subtype, samplerate, channels).** |
| 0x3a6e / 0x3bf0 | `SoundFile(path, const SF_INFO*, normalize)`: write with an explicit SF_INFO |
| 0x3178 | copy SF_INFO out |
| 0x30f2 | frames in range = end − start |
| 0x32b8 | tell = `sf_seek(0, SEEK_CUR)` − start |
| 0x36a4 | `read_double(buf, items)`: `sf_read_double(min(items, (end−start−tell)*channels))` |
| 0x362a | `readf_double` (frames) with the same bounding |
| 0x340e / 0x34c2 / 0x3576 | `read_short` / `read_int` / `read_float` (items) |
| 0x3832 / 0x374e | `write_double` / `writef_double`: throws runtime_error("error writing to "+path) on a short write |
| 0x3142 / 0x326a | from subtype (format & 0xffff): PCM_S8=1→8, PCM_16=2→16, PCM_24=3→24, PCM_32=4→32, PCM_U8=5→8 bits (sign/shift helpers) |

libsndfile 1.0.21 conversions with norm_double = true (the default, and also set explicitly here) for PCM.
This is from the libsndfile 1.0.21 source. The constants 32767.0, 1/32768, 0x7FFFFFFF and 1/2^31 are present in the bundled dylib.
* PCM_16 read: `d = s * (1.0/32768.0)`. PCM_16 write: `s = lrint(d * 32767.0)` (clipping is OFF by default, no SFC_SET_CLIPPING).
  **This is asymmetric.** A 16-bit file therefore gets a gain of 32767/32768 plus requantization on every pass through a stage, even where the stage itself is unity gain.
* PCM_24/PCM_32: the sample is read as a left-justified i32, then multiplied by `1/2^31`. Write: `lrint(d * 0x7FFFFFFF)`, then the
  top bytes are stored (for 24-bit, the low byte is dropped by an arithmetic shift).
* PCM_U8: read `(u - 128) / 128`; write `lrint(d*127) + 128`. PCM_S8: read `/128`, write `*127`.
* FLOAT/DOUBLE: a plain cast, with no scaling.
* `lrint` is round-half-to-even under the default FP mode.
