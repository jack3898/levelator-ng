# Step 10 — `normalize` (FUN_00040522 → 0x404cc)

Sources: `/Users/bsharpe/src/level/level/normalize/{CommandLine.cpp,NormalizeOperation.cpp}`.
Evidence (pseudocode re/fn/*.c + asm in bin/level.ann.asm):
0x40522/0x404cc (entry), 0x3ff38 + 0x3ecae (command line), 0x405d0 (CCLArgs defaults), 0x41e8e (run),
0x406c8 (NormalizeOperation ctor), 0x417d0 (Process), 0x40ade (analysis), 0x40192/0x4021c (GainCalc ctor),
0x403a2 (iterate), 0x40226 = 0x400ce + 0x3ea3c (gain curve), 0x40260 (level), 0x43182 (secant), 0x411dc (apply/write),
0x563fc `fromDB`. 0x3ebbc / 0x43214 only `snprintf` into a local buffer (debug, nothing printed). 0x432ba is
**reduce_loud's** command-line parser, not normalize's.

## Invocation (orchestrator asm 0x2c5fc–0x2c65d)

```
argc = 7
argv = { "normalize", "-l", "-18.0", "-b", sprintf("%d", CCLArgs.bit_depth), <base>_10<ext>, <output> }
```
- "-18.0" is the std::string global 0x1f1420 (static initializer at 0x168355, via ptr slot 0x1f33e8).
- `bit_depth` is the main program's own `--bit-depth,-b` option (CCLArgs+0, `value<int>` default **0**, FUN_00058296).
  The GUI runs `level <in> <out> <key>` without -b, so the pipeline passes **"-b 0"** ⇒ output keeps the input's format.
- Return: 0x404cc always returns 0; all failures are exceptions (propagate). Non-zero would give
  "Step 9 did not complete successfully" (sic, line 0x2b4) on stderr.

## Options (FUN_0003ecae; defaults = the CCLArgs fields set by 0x405d0)

| option | field | type | default |
|---|---|---|---|
| `help,h` | | | |
| `options-file,o` | local string | | (if given, file parsed as config; "error opening X" on failure) |
| `target-level,l` "target level (dB)" | +0x10 | double | **-18.0** |
| `fit-tolerance,t` "fit tolerance (dB)" | +0x18 | double | **0.01** (0x3f847ae147ae147b) |
| `attack,a` "attack (ms)" | +0x20 | int | **2000** |
| `release,r` "release (ms)" | +0x24 | int | **2000** |
| `noise-floor,f` "noise floor (dB)" | +0x00 | double | **-44.0** |
| `peak-output,p` "peak output level (dB)" | +0x08 | double | **-1.0** |
| `bin-size,s` "bin size (ms)" | +0x28 | int | **50** |
| `bit-depth,b` "bit depth of output" | +0x2c | int | **0** |
| `sound-file-in` (positional 1, hidden) | +0x30 | string | |
| `sound-file-out` (positional 2, hidden) | +0x34 | string | |

Validation: help or missing in/out → usage to stderr + throw `ss_command_line_error`. bit_depth must be one of
{0, 8, 16, 24, 32}, else stderr `"Unknown bit depth: <n>"` + throw.

## Derived values (NormalizeOperation ctor 0x406c8)
```
if peak_db > 0  { peak_db = -peak_db;  stderr "Warning: Peak must be negative.  Value has been changed to %.2lf." }
if noise_db > 0 { noise_db = -noise_db; stderr "Warning: Specified noise floor was positive.  Value has been changed to %.2lf." }
peak_lin   = fromDB(peak_db)     // +0x40   fromDB(x) = pow(10.0, x / 20.0)   (0x563fc)
noise_lin  = fromDB(noise_db)    // +0x38
target_lin = fromDB(target_db)   // +0x48
```
Pipeline: peak_lin = 10^(-1/20), noise_lin = 10^(-44/20), target_lin = 10^(-18/20).

## Process (0x417d0)
```
open input (SFM_READ) → info {frames, samplerate, channels, format}; failure → throw ss_sf_openinput_error
if channels >= 3 → throw ss_sf_too_many_channels_error
bin_frames  = (samplerate * bin_ms) / 1000          // int32
bin_samples = bin_frames * channels
nbins       = ceil(frames / bin_frames)              // 64-bit (frames + bin_frames - 1) / bin_frames
bins        = new CBin[nbins]  // {gain f64 @0, peak f64 @8, rms f64 @0x10}
overall_rms = analyse()                               // 0x40ade
sf_seek(in, 0, SEEK_SET)
if overall_rms == 0.0 → throw ss_sf_input_silent_error  (NaN passes)
gains = new f64[nbins + 1]
calc = GainCalc{ overall_rms, bins, nbins, nbins+1, noise_lin, peak_lin, gains,
                 attack_bins = attack_ms / bin_ms, release_bins = release_ms / bin_ms,   // int division: 40, 40
                 target_lin, target_db = 20*log10(target_lin), fit_tol }
calc.iterate()                                        // 0x403a2
apply_and_write()                                     // 0x411dc
```

## Analysis (0x40ade) — per-bin peak/RMS and overall RMS
Reads with **`sf_read_double`** (item count, not frames) `bin_samples` items per bin into an interleaved buffer.
```
tot_sq = 0.0; tot_n: i64 = 0
for b in 0..nbins:
    k = sf_read_double(in, buf, bin_samples);  if k == 0 → throw ss_sf_unexpected_eof
    pk = 0.0; s0 = 0.0; s1 = 0.0
    j = 0
    while j < k:                                  // signed int
        a = |buf[j]|;  if a > pk {pk = a};  s0 = a*a + s0
        if channels == 2 { j += 1; a = |buf[j]|; if !(a <= pk) {pk = a}; s1 = a*a + s1 }
        j += 1
    m = k / channels                              // integer division (idiv)
    rms = if channels == 1 { sqrt(s0 / m) }
          else { r0 = sqrt(s0/m); r1 = sqrt(s1/m); if r1 <= r0 { r0 } else { r1 } }   // max over channels
    bins[b] = {gain: 1.0, peak: pk, rms}
    if noise_lin < rms {                           // strict (ucomisd noise, rms; jae skip)
        tot_sq += if channels == 1 { s0 } else { s0 + s1 }    // tot_sq = tot_sq + (s0+s1)
        tot_n  += k                                // samples (all channels)
    }
    progress(91 → 95, frac = if nbins == 1 {1.0} else {b as f64 / (nbins-1) as f64})
overall_rms = if tot_n == 0 { 0.0 } else { sqrt(tot_sq / tot_n as f64) }
```
Samples are normalised doubles (libsndfile 1.0.21: PCM16 s/32768.0, PCM24 (s<<8)/2^31, PCM32 s/2^31, float as is).

## Gain curve for a given broadband gain G — 0x40226 = 0x400ce then 0x3ea3c

### 0x400ce — per-bin target gain
```
for b in 0..nbins:
    bins[b].gain = if noise_lin < bins[b].rms {                 // ucomisd noise,rms ; jb → limit branch
                       q = peak_lin / bins[b].peak;  if q <= G { q } else { G }   // cmovbe (NaN → q)
                   } else { G }                                  // quiet bins: NOT peak-limited
```
### 0x3ea3c — ramped gain envelope at bin boundaries (`gains[0..=nbins]`)
```
gains[0..nbins+1] = G                                            // 0x40156
for i in 0..nbins:
    b = bins[i].gain
    if b < G {                                                   // ucomisd G,b ; jbe skip
        // release side: indices i+1 .. end-1 , ramp b → G over (release+1) steps
        step = (G - b) / (release_bins + 1) as f64
        end  = min(i + 2 + release_bins, nbins + 1)
        v = b; for j in i+1 .. end { gains[j] = min(v, gains[j]); v = step + v }   // minsd v, g
        // attack side: indices i, i-1, ..., start+1 , ramp b → G backwards over (attack+1) steps
        step = (G - b) / (attack_bins + 1) as f64
        start = max(i - attack_bins - 1, -1)
        v = b; j = i; while j > start { gains[j] = min(v, gains[j]); j -= 1; v = step + v }
    }
```
`min(v, g)` = minsd semantics: `if v < g { v } else { g }`.
gains[i] is the gain at the start of bin i, gains[i+1] at its end (a reduced bin gets b at both edges).

## Resulting level (0x40260)
```
sum = 0.0; cnt = 0
for i in 0 .. nbins-1:                     // NOTE: last bin excluded (i < nbins-1)
    r = bins[i].rms
    if noise_lin < r { cnt += 1; v = r * ((gains[i] + gains[i+1]) * 0.5); sum = sum + v*v }
level_db = if sum == 0.0 || cnt == 0 { -200.0 } else { 10.0 * log10(sum / cnt as f64) }
```

## Secant iteration (0x403a2, 0x43182)
```
G_prev = 1.0
curve(1.0);  L_prev = level()
G = target_lin / overall_rms
loop {
    curve(G); L = level()
    G_next = secant(G_prev, G, L_prev, L, target_db)
    if |L - target_db| <= fit_tol { break }        // ucomisd tol, |diff| ; jb → continue (NaN → continue)
    G_prev = G; G = G_next; L_prev = L
}
// gains[] now holds the curve for the last G evaluated
fn secant(x0, x1, y0, y1, t) -> f64 { if y0 != y1 { x0 + ((t - y0) * (x1 - x0)) / (y1 - y0) } else { x0 } }
```
(all SSE2 double; `(t-y0)*(x1-x0)` multiplied first, then divided by `(y1-y0)`, then added to x0.)
target_db = `20.0 * log10(target_lin)` (≈ -18, not exactly).
**No iteration cap**: if the level stops responding to G (e.g. nbins ≤ 1, all analysed bins ≤ noise floor, or every
loud bin already peak-limited) the binary loops forever. Recommend an iteration cap (e.g. 1000) in the port, taking
the last evaluated curve.

## Apply and write (0x411dc)
```
if bit_depth != 0 {
    info.format = (info.format & 0xffff0000) | match bit_depth { 8 => 1 /*PCM_S8*/, 16 => 2, 24 => 3, 32 => 4 }
    // other values: stderr "Unknown bit depth: n" and format left unchanged (unreachable: validated earlier)
}
out = SoundFile(out_path, SFM_WRITE, info, norm_double = true)   // 0x3bf0→0x3a6e→0x3916: sf_open + sf_command(0x1012 SET_NORM_DOUBLE, 1)
                                     // failure → runtime_error "error opening sound file \"<path>\""
buf = new f64[bin_samples]
for i in 0..nbins:
    k = sf_readf_double(in, buf, bin_frames)          // frames
    g = gains[i]; d = (gains[i+1] - gains[i]) / (bin_frames as f64)   // once per bin
    for f in 0..k { for c in 0..channels { x = g * x }; g = g + d }   // mulsd g,x
    if sf_writef_double(out, buf, k) != k → throw ss_sf_io_error / runtime_error "error writing to <path>"
    progress(95 → 100, frac = if nbins == 1 {1.0} else {i as f64/(nbins-1) as f64})
```
- No clipping (SFC_SET_CLIPPING never issued), no dithering. libsndfile 1.0.21 PCM write (pcm.c, normalisation on, no clipping): 16-bit `lrint(x * 0x7FFF)`;
  24/32-bit scale by 0x7FFFFFFF then truncate to the container width (verify against the 1.0.21 source); overflow wraps. Loud bins are limited to
  peak_lin (-1 dBFS) by construction; bins under the noise floor get the full G and could in theory exceed 1.0.
- With "-b 0" the output container/subtype/samplerate/channels equal those of `<base>_10<ext>` (= original input's,
  since every earlier stage writes with its input's SF_INFO).
- Mono/stereo only.

## Progress constants (normal mode; the alternatives in parentheses belong to the disabled skip-preprocessing mode)
analysis 91.0 → 95.0 (84 → 89); write 95.0 → 100.0 (90 → 100).

## Rust skeleton
```rust
pub fn normalize(inp: &mut SndIn, out_path: &Path, target_db: f64 /*-18.0*/, bit_depth: i32 /*0*/) -> Result<()> {
    let (noise_db, peak_db, fit_tol, att_ms, rel_ms, bin_ms) = (-44.0, -1.0, 0.01, 2000, 2000, 50);
    let from_db = |x: f64| 10f64.powf(x / 20.0);
    let (noise, peakl, tl) = (from_db(noise_db), from_db(peak_db), from_db(target_db));
    let ch = inp.channels; if ch >= 3 { bail!(TooManyChannels) }
    let bf = (inp.samplerate * bin_ms) / 1000; let bs = bf * ch;
    let nb = ((inp.frames + bf as i64 - 1) / bf as i64) as usize;
    let (bins, rms) = analyse(inp, nb, bs, ch, noise);          // as above
    inp.seek(0); if rms == 0.0 { bail!(Silent) }
    let mut gains = vec![0.0; nb + 1];
    let tdb = 20.0 * tl.log10();
    let (ab, rb) = ((att_ms / bin_ms) as i64, (rel_ms / bin_ms) as i64);
    curve(&mut bins, &mut gains, 1.0, noise, peakl, ab, rb); let mut lp = level(&bins, &gains, noise);
    let (mut gp, mut g) = (1.0, tl / rms);
    loop { curve(&mut bins, &mut gains, g, noise, peakl, ab, rb); let l = level(&bins, &gains, noise);
           let gn = if lp != l { gp + ((tdb - lp) * (g - gp)) / (l - lp) } else { gp };
           if (l - tdb).abs() <= fit_tol { break }  // NaN → keep looping, like the binary
           gp = g; g = gn; lp = l; }
    write_with_gains(inp, out_path, &gains, bf, ch, bit_depth)
}
```
