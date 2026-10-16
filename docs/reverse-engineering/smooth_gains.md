# Step 8 — `smooth_gains` (FUN_0005117a, helper FUN_00050efc "SmoothGains1")

Source path strings: `/Users/bsharpe/src/level/level/smooth_gains/smooth_gains.cpp`.
Evidence: re/fn/05117a.c, re/fn/050efc.c, asm 0x5117a–0x5253f and 0x50efc–0x51178 (bin/level.ann.asm).

## Invocation from the orchestrator (asm 0x2bf47–0x2bfb2)

```
argc = 7
argv = { "smooth_gains", "-a", "40", "-b", "2", <base>_08.txt, <base>_09.txt }
```
- `[ebp-0x184]` = `<base>_08.txt` = the **gains file written by calculate_gains** (`--gains-file`), see calculate_gains.md
  (the brief's _07/_08 labelling is swapped; _08 is the gains file, _07 the rms file).
- `[ebp-0x188]` = `<base>_09.txt` = output.
- Return value 0 = OK, non-zero → orchestrator prints "Step 8 did not complete successfully" to stderr and returns 1.

## Options (boost::program_options, asm 0x51293–0x513e4)

| option | var | type | default |
|---|---|---|---|
| `help,h` | – | – | – |
| `attack-increasing,a` "attack for increasing gain" | ebp-0x88 | int | **10** |
| `attack-decreasing,b` "attack for decreasing gain" | ebp-0x84 | int | **2** |
| `input-file,i` (positional #1) | string | | |
| `output-file,o` (positional #2) | string | | |

Pipeline values: attack_inc = **40**, attack_dec = **2** (units = bins; bins are 50 ms → 2.0 s / 0.1 s).
Usage error (help given, argc < 3, or input/output missing): prints the options description to stderr, returns 1.

## File reading (asm 0x51d05–0x51f74)

1. `ifstream in(input)`; if not open or fail → stderr `"Error opening " + input`, return 1.
2. Capacity pass: `while(in.good()) { in.ignore(256, '\n'); cap++; }` (cap = lines+1, starting count 0). Then `clear(); seekg(0)`.
   If still bad → `"Error: <input> is not good for i/o"`, return 1.
3. Allocate `raw = new double[cap]`, `n = 0`.
4. `while (in.good()) { in.getline(buf, 256); v = atof(buf); if (v != 0.0) raw[n++] = v; }`
   - **Values equal to 0.0 are dropped** (`ucomisd v, 0.0; jp keep; je skip` → NaN is kept). The last getline at EOF yields "" → atof = 0 → dropped.
   - Input values are the gains written by calculate_gains with `ostream << double` (6 significant digits, `%g` style),
     i.e. **linear** gains. A Rust port that keeps the gains in memory must round them to 6 significant digits the
     same way (format `%g`, precision 6, then parse) to be bit-faithful.
5. `in.close()`.

## Algorithm

```
fwd  = new double[n];  ramp(raw, fwd, n, attack_inc)       // 0x51fa6, attack = [ebp-0x88] (40)
                       // returns non-zero only in an impossible branch → "Error in SmoothGains1()", return value stays 0/garbage
reverse(raw[0..n])                                          // in place, 0x5213f loop (swap i, n-1-i)
bwd  = new double[n];  ramp(raw_reversed, bwd, n, attack_dec)  // 0x52190, attack = [ebp-0x84] (2)
for i in 0..n:                                              // 0x521af
    b = bwd[n-1-i]
    out[i] = if fwd[i] > b { b } else { fwd[i] }            // ucomisd fwd,b ; cmova → min, NaN keeps fwd
```

### `ramp(in, out, n, attack)` = FUN_00050efc (exact asm semantics)

Only *increases* are rate-limited; decreases are followed immediately (in the reversed pass that becomes "decreases are anticipated").

```rust
fn ramp(inp: &[f64], out: &mut [f64], attack: i32) -> i32 {
    // NOTE: the binary unconditionally does out[0] = in[0] (even for n == 0 → UB); guard n == 0 in Rust.
    out[0] = inp[0];
    let mut ramping = false;
    let mut step = 0.0f64;        // xmm2, initialised to 0.0
    let mut count: i32 = 0;       // [ebp-0xf0]
    for i in 1..inp.len() {       // unsigned compare i < n
        if ramping {
            let r = step + out[i-1];          // addsd: step + prev
            if r > inp[i] {                   // ucomisd r, in ; jbe → else (NaN → else)
                out[i] = inp[i];              // overshoot → snap to target, stop ramp
                ramping = false;
            } else {
                out[i] = r;
                count -= 1;
                if count <= 0 { ramping = false; }   // "jg continue" keeps ramping while count > 0
            }
        } else {
            let x = inp[i]; let p = out[i-1];
            if x > p && attack != 1 {         // ucomisd x,p ; jbe → plain copy ; NaN → plain copy
                step = (x - p) / (attack as f64);
                count = attack - 1;
                out[i] = p + step;
                ramping = true;
            } else {
                out[i] = x;
            }
        }
    }
    0
}
```
Notes:
- `step` and `count` are computed once at ramp start; the target `inp[i]` may change during the ramp (the ramp only checks
  against the current `inp[i]` and snaps down if exceeded). When `count` reaches 0 the ramp ends even if `out` has not
  reached `inp[i]` (next sample then starts a fresh ramp if still below).
- With attack = 40, an upward step reaches its target after exactly 40 bins (unless snapped).
- attack ≤ 0 is not guarded (division by 0 / negative count); not reachable from the pipeline.

## Output (asm 0x521d3–0x523e6)

`ofstream out(output)`; on failure → `"Error opening <output> for output"`, return 1.
For each value: `gcvt(v, 128, buf); out << buf << "\n";`. 128 significant digits ⇒ the text is the exact (or ≥17-digit)
decimal expansion, so it round-trips to the identical double. In Rust write `format!("{:?}", v)`/`{}` (shortest
round-trip) — the downstream reader (apply_gains, istream >> double) recovers the identical f64.
Returns 0.

## Edge cases
- n == 0 (all gains zero / empty file): binary writes out[0] of a 0-length array (heap UB) then writes an empty file.
  Port: write empty file, return 0. (Downstream: apply_gains writes 0 frames → normalize throws "input silent".)
- n == 1: output = input.
- NaN inputs are kept and propagate per the comparisons above.

## Rust reference

```rust
pub fn smooth_gains(raw_gains_6sig: &[f64], attack_inc: i32 /*40*/, attack_dec: i32 /*2*/) -> Vec<f64> {
    let g: Vec<f64> = raw_gains_6sig.iter().copied().filter(|&v| v != 0.0).collect();
    let n = g.len();
    if n == 0 { return vec![]; }
    let mut fwd = vec![0.0; n]; ramp(&g, &mut fwd, attack_inc);
    let rev: Vec<f64> = g.iter().rev().copied().collect();
    let mut bwd = vec![0.0; n]; ramp(&rev, &mut bwd, attack_dec);
    (0..n).map(|i| { let b = bwd[n-1-i]; if fwd[i] > b { b } else { fwd[i] } }).collect()
}
```
