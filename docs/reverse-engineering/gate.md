# Step 2 — `gate` (noise gate; FUN_00039f10 → FUN_00039840)

Status: implementation-ready. Evidence comes from `re/fn/*.c`, the new decompiles in `work_gate/dec.json`
(CNoiseGate ctor 0x3a7b4) and `bin/level.ann.asm`. All units below are **samples per channel (frames)**
unless stated otherwise.

## 1. Orchestrator call (FUN_00028dfc @ 0x2a92a..0x2aa99)

```
L   = return value of highpass (dB RMS of filtered signal, see highpass.md §5)
thr = min(-40.0, L - 18.0)                  // subsd 18.0 (@0x1a6f78), minsd with -40.0 (@0x1a6f80)
buf = sprintf("%.1lf", thr)                 // e.g. "-40.0", or "-47.3" when L = -29.3
argv = ["gate",
  "--gate__attenuation_max",   *0x1f1448 = "-12.0",
  "--gate__attenuation_min",   *0x1f1444 = "0.0",
  "--gate__threshold_attack",  buf,
  "--gate__threshold_release", buf,
  "--gate__attack",            *0x1f1438 = "250",
  "--gate__release",           *0x1f1434 = "750",
  "--gate__hold_time",         *0x1f1430 = "250",
  "--gate__buffer_size",       *0x1f142c = "5",
  "--gate__anticipate_attack", *0x1f1428 = "250",
  "--gate__verbosity",         *0x1f1424 = "0",
  <out>_01, <out>_02]                       argc = 0x17 = 23
```
A non-zero return gives "Step 2 did not complete successfully". **The "-44.0" globals (0x1f1440, 0x1f143c) are
NOT used by the gate call.** Both thresholds are the data-derived `buf`. Because the value goes through
`%.1lf` and back through boost lexical_cast, the threshold in dB is `thr` rounded to one decimal place
(exact-decimal rounding, as in Rust's `format!("{:.1}")`).

## 2. GateArgs (default ctor FUN_00038700, parser FUN_000387d0, derive FUN_000385be)

| off | type | option | meaning |
|---|---|---|---|
| +0x00 | f64 | `gate__attenuation_max,x` "max attenuation (dB)" | -12.0 |
| +0x08 | f64 | derived | `fromDB(att_min)` = 1.0 → **gain when open** (CMultiplier max) |
| +0x10 | f64 | `gate__attenuation_min,n` "min attenuation (dB)" | 0.0 |
| +0x18 | f64 | derived | `fromDB(att_max)` = 10^(-0.6) → **gain when closed** (CMultiplier min) |
| +0x20 / +0x24 | i32 | `gate__attack,a` (ms) / samples | 250 |
| +0x28 / +0x2c | i32 | `gate__release,r` (ms) / samples | 750 |
| +0x30 / +0x34 | i32 | `gate__hold_time,d` (ms) / samples | 250 |
| +0x38 / +0x40 / +0x48 | f64 | `gate__threshold_attack,t` dB / linear / energy | buf |
| +0x50 / +0x58 / +0x60 | f64 | `gate__threshold_release,t` dB / linear / energy | buf |
| +0x68 / +0x6c | i32 | `gate__anticipate_attack,k` (ms) / samples | 250 |
| +0x70 / +0x74 | i32 | `gate__buffer_size,b` "buffer size for level meter (ms)" / samples | 5 |
| +0x78 / +0x7c | str | sound-file-in / sound-file-out | |
| +0x84 | i32 | `gate__verbosity,v` | 0 |

The option→offset mapping comes from the reverse construction order of the `value<>()` calls. It is consistent with the types and with the uses in 0x385be and 0x39cb7.
Built-in defaults (not used by the orchestrator): att_max -12, att_min 0, thresholds -39/-39, attack 100,
release 250, hold 250, buffer 5.

Derivation (FUN_000385be, `sr = SF_INFO.samplerate`, all integer arithmetic in i32, `/1000` truncates toward 0):
```
attack_n   = (sr * attack_ms)   / 1000      // 44100 → 11025
release_n  = (sr * release_ms)  / 1000      // 44100 → 33075
hold_n     = (sr * hold_ms)     / 1000      // 44100 → 11025
antic_n    = (sr * antic_ms)    / 1000      // 44100 → 11025
N          = (sr * buffer_ms)   / 1000      // 44100 → 220, 48000 → 240
fromDB(x)  = pow(10.0, x / 20.0)            // FUN_000563fc, f64 20.0 @0x1a6fa0
g_open     = fromDB(att_min)    // 1.0
g_closed   = fromDB(att_max)    // 0.251188643150958
thrA_lin   = fromDB(thrA_dB);  thrA = (N as f64) * (thrA_lin * thrA_lin)   // exact op order: N * (l*l)
thrR_lin   = fromDB(thrR_dB);  thrR = (N as f64) * (thrR_lin * thrR_lin)
```
All levels are compared as **sums of squares over N samples**. This is equivalent to comparing the RMS over N
samples against the linear threshold, but the port must compare the raw sums to stay bit-exact.

## 3. Objects

**CLevelMeter** (ctor 0x3a752, 0x24 bytes): `level:f64 @0, N:i32 @8, buf:shared_array<f64>[N] @0xc,
cur:*f64 @0x14 (= buf[0]), last:*f64 @0x18 (= &buf[N-1]), sum:f64 @0x1c`. The buffer is NOT zeroed, but it is always
prefilled before use (below).
* `prefill(data)` (FUN_0003aa42): `sum = 0; for i in 0..N { buf[i] = data[i]; sum = data[i]*data[i] + sum }; level = sum`.
  The `cur` pointer is not reset, so it stays at buf[0].
* `push(x)` (FUN_0003aac4): `sum = sum - (*cur)*(*cur); *cur = x; cur += 1; sum = x*x + sum; if cur > last { cur = buf }; level = sum; return sum`.
  This is a running sum with floating-point drift. Reproduce the order exactly: subtract first, then add `x*x + sum`.

**CMultiplier** (ctor 0x3a026, called as `CMultiplier(g_open, g_closed, attack_n, release_n, 1.0)`; the 5th argument is unused):
`max @0 = g_open, min @8 = g_closed, up @0x10 = (max - min)/(attack_n as f64), down @0x18 = (max - min)/(release_n as f64), gain @0x20`.

**CNoiseGate** (ctor 0x3a7b4): `thrA @0x10, thrR @0x18, sf_main @0x20, sf_ahead @0x28, sf_out @0x30,
meter curL @0x38, aheadL @0x5c, aheadR @0x80, curR @0xa4, multL @0xc8 (gain @0xe8), multR @0xf0 (gain @0x110),
antic_n @0x118, attack_n @0x11c, holdcntL @0x120, holdcntR @0x124, hold_n @0x128, total_items i64 @0x12c,
release_n @0x134, verbosity @0x138, bufMain f64[8192] @0x13c, bufAhead f64[8192] @0x1013c, in name @0x2013c,
out name @0x20140, stateL @0x20144, stateR @0x20148`.

## 4. Driver FUN_00039840

```
args = parse(argv); 
h_main  = sf_open(in,  SFM_READ, &G_INFO /*0x1f13ac*/)   else throw ss_sf_openinput_error
h_ahead = sf_open(in,  SFM_READ, &G_INFO)                else throw ss_sf_openinput_error
h_out   = sf_open(out, SFM_WRITE, &G_INFO)               else throw ss_sf_openoutput_error
          // same SF_INFO as the input → same container/subtype/sr/channels; no SFC_SET_NORM_DOUBLE
          // call is made, so libsndfile's default normalisation (true) applies, as in highpass.md §7
derive(args, sr = G_INFO.samplerate)
if sf_seek(h_ahead, antic_n /*frames*/, SEEK_SET) == -1 {
    close h_main, h_ahead (and h_out); copy input file → output byte-for-byte (FUN_00028d04: remove out if it exists, then boost copy_file)
    return 0
}
gate = CNoiseGate(...); total_items = frames*channels
return if channels == 1 { run_mono() } else { run_stereo() }   // FUN_0003bf6f; returns 0
```
Exceptions (unexpected EOF, seek or write errors) propagate to the orchestrator, and the step then fails.
There is no channel-count check here: anything other than 1 channel is treated as stereo. Highpass already rejects more than 2 channels.

## 5. Initialisation (run_mono 0x3b008 / run_stereo 0x3b66a)

Mono (FUN_0003aea6, called twice with meter curL then aheadL). Each call reads N items from **h_main** (not
h_ahead!), checks the read is not short (else ss_sf_unexpected_eof), `sf_seek(h_main, -N, SEEK_CUR)`, and prefills that meter.
Both meters therefore start with x[0..N-1] and have equal levels S0.

Stereo (FUN_0003ac80, called as (curL, curR) and then (aheadL, aheadR)) reads 2N items from h_main,
seeks back N frames, de-interleaves, and prefills the L meter with L[0..N-1] and the R meter with R[0..N-1].

Initial state per channel c (asm 0x3b056..0x3b0b9):
```
if thrA < ahead_level || thrA < cur_level { state = OPEN(0);   gain = g_open   }
else                                      { state = CLOSED(1); gain = g_closed }
```

## 6. Per-sample state machine (identical for mono and for each stereo channel; verified in asm 0x3b2c8..0x3b422)

States: 0 OPEN, 1 CLOSED, 2 ATTACK (opening), 3 RELEASE (closing), 4 HOLD.
For each input sample x (channel c), with `cur = curMeter.push(x)` and `ahd` = lookahead level (§7):
```
match state {
 0 OPEN:    if cur < thrR                      { state = HOLD; holdcnt = 0 }
 1 CLOSED:  if ahd > thrA                      { state = ATTACK }
 2 ATTACK:  if ahd < thrR                      { state = RELEASE }
            else { gain = gain + up;   if gain >= g_open   { state = OPEN;   gain = g_open } }
 3 RELEASE: if ahd > thrA                      { state = ATTACK }
            else { gain = gain - down; if gain <= g_closed { state = CLOSED; gain = g_closed } }
 4 HOLD:    if cur > thrR                      { state = OPEN }
            else { holdcnt += 1; if holdcnt > hold_n { state = RELEASE } }
}
out = x * gain        // gain AFTER this sample's update
```
Notes:
* OPEN and HOLD use the trailing (current) meter. CLOSED, ATTACK and RELEASE use the look-ahead meter.
* The gain ramp is **linear in amplitude**. ATTACK goes from g_closed up to g_open in `attack_n` samples
  (step `(1-0.2512)/11025`). RELEASE goes down in `release_n` samples. A ramp can reverse mid-way.
  Gain changes only in states 2 and 3, and never on the same sample as a state change caused by a threshold test.
* The full-scale behaviour is a 12 dB "downward gate" (attenuation, not muting): open gain 1.0 (0 dB), closed gain −12 dB.
* All comparisons are strict as written (`<`, `>`), and the ramp end tests are `>=` and `<=`.
  With NaN levels every test is false (ucomisd semantics as above).

## 7. Block loop and the look-ahead stream

```
block = 8192 - 8192 % channels            // items
n_main  = sf_read_double(h_main,  bufMain,  block)
n_ahead = sf_read_double(h_ahead, bufAhead, block)
while n_main >= 1 {
   mono:   for i in 0..n_main {                         // processes exactly n_main items
              x = bufMain[i]; cur = curL.push(x)
              ahd = if n_ahead >= 1 { aheadL.push(bufAhead[i]) } else { cur }   // aheadL NOT pushed if n_ahead < 1
              step(L, x, cur, ahd); bufMain[i] = x * gainL }
   stereo: i = 0; loop { xl = bufMain[i]; xr = bufMain[i+1]
              curl = curL.push(xl); curr = curR.push(xr)
              if n_ahead >= 1 { ahl = aheadL.push(bufAhead[i]); ahr = aheadR.push(bufAhead[i+1]) } else { ahl = curl; ahr = curr }
              step(L, ...); step(R, ...)                 // L state machine first, then R; they are fully independent
              bufMain[i] = xl*gainL; bufMain[i+1] = xr*gainR
              i += 2; if !(i < n_main) break }
   if sf_write_double(h_out, bufMain, n_main) != n_main { throw ss_sf_write_error }
   n_main  = sf_read_double(h_main,  bufMain,  block)
   n_ahead = sf_read_double(h_ahead, bufAhead, block)
}
return 0
```
* h_ahead is `antic_n` frames ahead, so at sample i the look-ahead meter window ends at x[i + antic_n]. The
  look-ahead meter starts prefilled with x[0..N-1] and its ring pointer at slot 0, so for i < N-1 its
  window mixes x[antic_n..antic_n+i] with x[i+1..N-1]. Simulate the ring buffer exactly.
* **Quirk (must be reproduced for bit-exactness):** the "look-ahead exhausted" test is per block (`n_ahead >= 1`), not per sample.
  In the block where h_ahead returns a short count `0 < n_ahead < n_main`, indices `i >= n_ahead` read **stale data
  left in bufAhead from the previous block** (the samples at the same index one block earlier, i.e. x[i + antic_n − 8192] in mono).
  Port: keep a persistent `bufAhead: [f64; 8192]` and overwrite only the first `n_ahead` entries.
  If this happens in the very first block (file shorter than about 8192 + antic_n items), those entries are
  uninitialised stack memory in the original. Use 0.0 as the most plausible stand-in.
  From the next block on (`n_ahead == 0`), `ahd = cur` and the look-ahead meters freeze.
* The output has exactly as many items as the input, and the sample format is the input's (libsndfile re-quantisation as in highpass.md §7).
* Progress callbacks (FUN_0005b166) and verbosity printing (FUN_000386ca is empty) do not affect output.

## 8. Rust-like reference

```rust
struct Meter { buf: Vec<f64>, cur: usize, sum: f64 }
impl Meter {
  fn prefill(&mut self, d: &[f64]) { self.sum = 0.0; for i in 0..self.buf.len() { self.buf[i] = d[i]; self.sum = d[i]*d[i] + self.sum; } }
  fn push(&mut self, x: f64) -> f64 { let o = self.buf[self.cur]; self.sum = self.sum - o*o; self.buf[self.cur] = x;
      self.cur += 1; self.sum = x*x + self.sum; if self.cur == self.buf.len() { self.cur = 0; } self.sum }
}
#[derive(Clone, Copy)] enum St { Open, Closed, Attack, Release, Hold }
struct Chan { st: St, gain: f64, hold: i32 }
fn step(c: &mut Chan, cur: f64, ahd: f64, p: &P) {
  match c.st {
    St::Open    => if cur < p.thr_r { c.st = St::Hold; c.hold = 0; },
    St::Closed  => if ahd > p.thr_a { c.st = St::Attack; },
    St::Attack  => if ahd < p.thr_r { c.st = St::Release; } else { c.gain = c.gain + p.up;
                      if c.gain >= p.g_open { c.st = St::Open; c.gain = p.g_open; } },
    St::Release => if ahd > p.thr_a { c.st = St::Attack; } else { c.gain = c.gain - p.down;
                      if p.g_closed >= c.gain { c.st = St::Closed; c.gain = p.g_closed; } },
    St::Hold    => if cur > p.thr_r { c.st = St::Open; } else { c.hold += 1; if c.hold > p.hold_n { c.st = St::Release; } },
  }
}
// p.up = (g_open - g_closed) / attack_n as f64; p.down = (g_open - g_closed) / release_n as f64
// init: st = if p.thr_a < ahead0 || p.thr_a < cur0 { (Open, g_open) } else { (Closed, g_closed) }
```

## 9. Edge cases
* antic_n > frames: the seek fails, so the output is a byte copy of the input and the gate is skipped (returns 0).
* File shorter than N frames: the prefill read is short, ss_sf_unexpected_eof is thrown, and step 2 fails.
* attack_n or release_n == 0: the step becomes +inf, so the gain jumps to the end value in one sample. This is not reachable with the defaults.
* sr*ms overflows i32 only for absurd values.
