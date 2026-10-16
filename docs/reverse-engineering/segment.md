# Step 6 — wrap_segment (FUN_00055774) + segment (FUN_0004f920 / FUN_0004da82)

Source files named in the binary: `level/wrap_segment/wrap_segment.cpp`, `level/segment/segment.cpp`,
`segment/CostCalculator.cpp|.h`, `segment/CommandLine.cpp`, `segment/segment_util.cpp`.
Everything below comes from `bin/level.ann.asm`. The stub→symbol mapping comes from the Mach-O indirect symbol table
(the `stubs.py` script writes it to `stubs.txt`).

## 1. Call from the orchestrator (FUN_00028dfc @ 0x2b6d3–0x2b821)

```
wrap_segment(
  rms_file    : std::string  = "<out>_05.txt"           // [ebp+0x08]
  unused1     : std::string  = "whereisthisused"        // [ebp+0x0c]  never read
  unused2     : std::string  = "..\\options_segment.txt" // [ebp+0x10]  never read
  clt_file    : std::string  = "<out>_06.clt"           // [ebp+0x14]
  quiet_delta : f64          = -12.0  (0xC028000000000000)  // [ebp+0x18]
  samplerate  : i32          = SF_INFO.samplerate of the input file ([ebp-0x1e0] in orchestrator = sfinfo+8)
  bin_size_s  : f64          = 0.05   (0x3FA999999999999A)   // [ebp+0x24]
) -> i32 (0 = ok, 1 = error)
```
The `"..\options_segment.txt"` and `"whereisthisused"` arguments are dead. wrap_segment never reads them.

## 2. wrap_segment (0x55774)

```rust
fn wrap_segment(rms_file, clt_file, quiet_delta: f64, sr: i32, bin_s: f64) -> i32 {
    if quiet_delta > 0.0 { error("error: quiet_delta must be negative"); return 1; }  // ucomisd/jbe: 0.0 is accepted
    // a) read all whitespace-separated doubles (istream_iterator<double>) from rms_file -> deque v (dB values)
    // b) mean of the linear values:
    let mut sum = 0.0f64;
    for x in &v { sum += 10f64.powf(x / 20.0); }      // fromDB(x)=pow(10.0, x/20.0) @0x563fc, sequential sum
    let n = v.len() as u32 as f64;                     // unsigned -> double
    let thr = quiet_delta + 20.0 * log10(sum / n);     // computed as (log10(..)*20.0), then quiet_delta + that
    // c) keep the bins that are strictly above thr. Keep the ORIGINAL dB VALUE (not linear) and its index
    let (mut kept_idx, mut kept_val) = (vec![], vec![]);
    for i in 0..v.len() { if v[i] > thr { kept_idx.push(i as u32); kept_val.push(v[i]); } } // NaN not kept
    // d) serialise kept_val to a string: ostream_iterator<double>(oss, "\n") with DEFAULT stream formatting
    //    = printf("%.6g") per value, each followed by '\n'. segment re-parses this text, so the
    //    segmentation input is the dB value rounded to 6 significant digits.
    let text: String = kept_val.iter().map(|x| format_g6(*x) + "\n").collect();
    // e) run segment with argv = ["segment","--tolerance","3.5","--min-segment-length","20",
    //    "--max-breakpoints","30","--chunk-size","2400","--cost-function","SSD0"]   (argc = 0xb)
    //    The strings are the globals 0x1f13f4/0x1f13f0/0x1f13ec/0x1f13e8/0x1f13e4 set in the static init (si.ann.asm 0x1686f2..).
    let (fit_text, break_text) = segment(argv, text)?;   // non-zero -> return 1
    // f) fit_text is parsed into a deque<double> and then IGNORED.
    // g) break_text is parsed as ints (istream_iterator<int>) -> bps. Map each one back to an original bin index:
    let orig: Vec<u32> = bps.iter().map(|&b| kept_idx[b as usize]).collect();  // unchecked deque operator[]
    // h) write the CLT file (ofstream mode out|trunc = 0x30, '\n' line endings, no CR)
    let hop: i32 = (sr as f64 * bin_s) as i32;          // cvttsd2si = truncation. 44100 -> 2205, 48000 -> 2400, 22050 -> 1102
    write!(f, "\x03 CueListTool 1.3 file - do not edit unless you know exactly what you do!\n");
    write!(f, "Prefix=\n");
    write!(f, "{}\n", sr);                               // ostream<<(int) + endl
    for o in orig {
        write!(f, "{}\n", (o as u32).wrapping_mul(hop as u32)); // imul 32-bit, ostream<<(unsigned long) + endl
        write!(f, "0\n\n\n\n");
    }
    0
}
```
So each cue is the text `"<pos>\n0\n\n\n\n"`: the decimal position line, then `0`, then three empty lines.
The file ends right after the last `"0\n\n\n\n"`. With no breakpoints the file holds only the 3 header lines.
Example for 44.1 kHz:
```
\x03 CueListTool 1.3 file - do not edit unless you know exactly what you do!\n
Prefix=\n
44100\n
110250\n0\n\n\n\n
...
```
The first byte is 0x03, followed by a space. The header string literal is `"\x03 CueListTool 1.3 file - do not edit unless you know exactly what you do!\n"`.

## 3. segment (0x4f920 → FUN_0004da82)

`0x4f920(argc, argv, std::string data, ostream& fFit, ostream& fBreak)` copies the string and calls 0x4da82.

### 3.1 Options (CommandLine.cpp, FUN_0004b502; CCLArgs layout at &args)
| option | type | CCLArgs offset | value used here |
|---|---|---|---|
| `tolerance,t` "threshold of fit (0=no threshold)" | f64 | +0x0 | 3.5 |
| `chunk-size,k` "chunk size for processing in pieces (0=no chunking)" | u16 | +0x8 | 2400 |
| `max-breakpoints,m` "maximum number of breakpoints (0=no max)" | u16 | +0xa | 30 |
| `min-segment-length,s` | u16 | +0xc | 20 |
| `cost-function,c` "cost function (SSD0, MSD0)" | string | +0x10 | "SSD0" |
If both max-breakpoints and tolerance are unset, it errors with "One of max-breakpoints or tolerance must be set".

### 3.2 Calculator objects
`CCostCalculator` base (ctor 0x4ff1f): `+0 vtbl, +4 istream*, +8 u16 n (chunk length), +0xa u16 minseg, +0xc CostCombiner*, +0x10 name string`.
`CalculatorData0` sits at +0x14: `+0 double* pSum, +4 double* pSumSq` (scoped_arrays).

CalculatorData0 ctor (0x50470) → 0x4d2f8(n, istream, pSum, pSumSq):
* readSums 0x4d23e reads exactly `n` doubles with `istream >> double`. If the stream fails it throws read_sums_error.
* `pSum = scanl1(+, x)`: out[0]=x[0], then out[k]=out[k-1]+x[k] in order (0x4d5e8).
* `pSumSq = scanl1(+, x[k]*x[k])`.

```
RangeSum(i,j)   = i==0 ? S[j]  : S[j]  - S[i-1]          // 0x4d72c
RangeSumSq(i,j) = i==0 ? Q[j]  : Q[j]  - Q[i-1]          // 0x4d79c
RangeSSD(i,j)   = RangeSumSq(i,j) - (RangeSum(i,j)^2) / ((j-i+1) as u16 as f64)   // 0x4d81c
     (evaluated as: s=RangeSum; q=RangeSumSq; q - (s*s)/len)
```
All values are stored to memory as f64 between steps, so no x87 extended precision leaks in.

Vtables (__DATA 0x1e9a20 SSD0, 0x1e99c0 MSD0):
| slot | SSD0 (used) | MSD0 |
|---|---|---|
| 0 DP step | 0x4d892 | 0x4d892 |
| 1 cost(i,j) | 0x4d976 = RangeSSD(i,j) | 0x4d6da = RangeSSD(i,j)/(u16)(j-i+1) |
| 2 fit(i,j) | 0x4d996 = RangeSum(i,j)/(j-i+1) (mean) | 0x4d61e (same) |
| 4 normalise(c) | 0x4da24 = sqrt(c / n) | 0x4d6ac = sqrt(c) |
Combiner: SSD0 uses a `SumCostCombiner` (0x4ff54: `a+b`, computed as `cost + best[i]`). MSD0 uses a `MaxCostCombiner` (0x4ff92).
The makers are 0x50a8c (SSD0) and 0x50b6a (MSD0). `CCostCalculatorSD` is the shared base, with ctor 0x509ec.
For SSD0 the tolerance measure is therefore the **RMS residual in dB**, `sqrt(total_SSD / n)`.

### 3.3 Main algorithm (0x4da82). Data are the dB values, segmented directly with no log or linear conversion.
```rust
fn segment(text: &str, tol: f64 /*3.5*/, mut chunk: u16 /*2400*/, mut maxbp: u16 /*30*/, mut minseg: u16 /*20*/,
           fit: &mut String, brk: &mut String) -> i32 {
    // count the doubles in the text (stop at the first parse failure), then rewind
    let N: u32 = count_doubles(text);
    minseg = max(minseg, 1);                                   // 0x4df97
    let c = if (chunk as u32) <= N { chunk } else { N as u16 }; // 0x4dfbb
    let nchunks: u16;
    if c == 0 { nchunks = 1; chunk = N as u16; } else { nchunks = ((N - 1 + c as u32) / c as u32) as u16; chunk = c; }
    clamp_maxbp(&mut maxbp, chunk, minseg);                    // FUN_0004b486, see below
    // (MSD0 + minseg != 1 prints a warning only)
    let mut best_i: Vec<Vec<u16>> = vec![vec![]; maxbp as usize + 1];   // "ppBestIMatrix", (maxbp+1) pointers
    let mut best = vec![0f64; chunk as usize];                          // "pBestCost", chunk doubles
    progress(0.0);                                             // prints "35%"
    let mut offset: u32 = 0;
    let mut stream_pos = 0;                                    // values are consumed sequentially: chunks are contiguous, with no overlap
    for ci in 0..nchunks {
        if ci == nchunks - 1 {
            let r = (N % chunk as u32) as u16;
            chunk = if r == 0 { chunk } else { r };            // u16 truncation
            clamp_maxbp(&mut maxbp, chunk, minseg);            // maxbp only ever shrinks and carries over
        }
        if chunk < minseg { break; }                           // a trailing short chunk is dropped entirely
        let n = chunk;
        let x = read_next(n);                                  // CalculatorData0: prefix sums of this chunk only
        let mut err = sqrt(ssd(0, n-1) / n as f64);            // vfunc4(0,n-1) then vfunc16
        // (if err <= tol the code only snprintf("%5.2lf") into a local buffer, which has no effect)
        let mut k: u16 = 0;
        while k <= maxbp && err > tol {                        // ucomisd ja: NaN stops the loop
            if k == 0 {
                for j in (minseg-1)..n { best[j] = ssd(0, j); }
            } else {
                best_i[k] = vec![UNINIT; n];                   // new u16[n]
                let lo = ((k as u32 + 1) * minseg as u32 - 1) as u16;   // 16-bit imul
                let mut j = n;
                while { j -= 1; j >= lo } { dp_step(&mut best, &mut best_i[k], j, k); } // j = n-1 down to lo (in place)
            }
            err = sqrt(best[n-1] / n as f64);
            k += 1;
        }
        let nbp: u16 = if k == 0 { 0 } else { k - 1 };         // # breakpoints chosen
        // backtrack
        let mut starts = vec![0u16; nbp];                      // "pBreakpoints" (local segment starts)
        let mut gl = vec![0u32; nbp+1];                        // "pBreakpointsAdjusted" (global)
        gl[0] = offset;
        let mut j = n - 1;
        for kk in (1..=nbp).rev() {
            let i = best_i[kk][j];
            starts[kk-1] = i + 1;
            gl[kk] = offset + 1 + i as u32;
            j = i;
        }
        // write breakpoints (segment_util.cpp 0x50c60): every element of gl except zeros, as "<u32>\n"
        for &g in &gl { if g != 0 { writeln!(brk, "{}", g); } }
        // fit output (the per-sample segment mean, "%g" + endl). wrap_segment ignores it
        let mut s = 0;
        for &e in &starts { let m = mean(s, e-1); for _ in s..e { writeln!(fit, "{}", g6(m)); } s = e; }
        let m = mean(s, n-1); for _ in s..n { writeln!(fit, "{}", g6(m)); }
        offset += n as u32;
        progress(if nchunks == 1 { 1.0 } else { ci as f64 / (nchunks - 1) as f64 });
    }
    0
}

fn clamp_maxbp(maxbp: &mut u16, chunk: u16, minseg: u16) {   // 0x4b486
    let lim: u16 = (chunk / minseg).wrapping_sub(1);         // u16 arithmetic. chunk<minseg gives 0xFFFF
    *maxbp = if *maxbp == 0 { lim } else { min(*maxbp, lim) };
}

// 0x4d892  (vtable slot 0)
fn dp_step(best: &mut [f64], bi: &mut [u16], j: u16, k: u16) {
    let m = minseg as i32;
    let mut i = (k as i32) * m - 1;           // ((k-1)&0xffff)+1)*m - 1
    let mut have = false; let (mut bv, mut bidx) = (0.0, 0u16);
    while i < (j as i32) - m + 1 {            // i = k*m-1 ..= j-m  (signed compare)
        let c = best[i] + ssd((i+1) as u16, j); // SumCostCombiner(best[i], cost)
        if !have || bv > c { bv = c; bidx = i as u16; }   // strict '>' keeps the smallest i on ties
        have = true;
        i += 1;
    }
    best[j] = bv; bi[j] = bidx;
}
```
Notes on the recurrence:
* Level k holds `best_k[j]` = minimum total SSD of `x[0..=j]` split into k+1 segments, each at least `minseg` long.
  `best_k[j] = min_{i = k*m-1 ..= j-m} best_{k-1}[i] + SSD(i+1, j)`. The same `best` array is updated in place for
  j descending, so `best[i]` (i ≤ j-m < j) still holds the level k-1 value when it is read.
  Entries `best[j]` with j < (k+1)m-1 keep stale values from lower levels, but they are never read at level k+1
  because i ≥ (k+1)m-1 there.
* **How many breakpoints**: add one breakpoint at a time (k = 0,1,2,…). Stop at the first k whose optimal
  RMS residual `sqrt(best_k[n-1]/n)` is ≤ `tolerance` (3.5 dB), or after k = maxbp (30). If the
  whole chunk is already within tolerance, there are 0 breakpoints. maxbp is also capped at `n/minseg - 1`
  (u16 division), so (maxbp+1)*minseg ≤ n holds and every DP level is feasible.
* **Chunking**: the kept-value sequence is cut into consecutive, non-overlapping blocks of 2400 values. The last block holds
  `N % 2400` values, or 2400 if that is 0. Each block is segmented on its own, with its own prefix sums.
  Every chunk start offset except 0 is emitted as a breakpoint. If the last block is shorter than minseg (20) it is skipped and emits nothing.
  maxbp shrinks for the last chunk and stays shrunk (no effect, since that chunk is the last).
* **pBreakpointsAdjusted** = local breakpoint index + chunk offset. It starts with the chunk offset and is followed by
  `offset + start_of_segment_k` for k = 1..nbp, in ascending order. Breakpoints are segment START indices (the index just
  after the end of the previous segment) in the kept-value numbering. wrap_segment maps them through `kept_idx`.

### 3.4 Progress (FUN_0005b166)
`prog = {start, end, last=-1, cur=-1, cb=*(0x1f1394), ctx=*(0x1f1398)}`. The flag at `*(0x1f3374)` (→0x165aac, a constant 0)
selects start=35.0, end=69.0. Otherwise it would be 6.0..57.0. `pct = (int)floor(start + (end-start)*frac)`. If `pct > last`,
then `last = pct` and either `cb(pct/100.0f, ctx)` is called or `cout << pct << "%" << flush` is written (no newline). The function is called with frac=0 before the
loop and after every completed chunk with `ci/(nchunks-1)`, or 1.0 when there is one chunk. A cancel flag (*0x1a7010) throws.

### 3.5 Error/edge paths
* `N == 0` (nothing kept): chunk=0, so `0 < minseg` triggers the break. No output, return 0, and the CLT has only the header.
* `N < 20`: the single chunk is skipped, so there are no breakpoints.
* Fewer than 2400 kept values: one chunk, maxbp = min(30, N/20-1).
* readSums failure throws read_sums_error ("CostCalculator.cpp" line 0x39). The orchestrator treats non-zero as failure.
* An unknown cost function returns 1.

## 4. Things a Rust port must reproduce exactly
1. Segmentation runs on the **dB values** from `_05.txt`, filtered by `v > quiet_delta + 20*log10(mean(10^(v/20)))`
   with quiet_delta = -12.0. In other words, it keeps only bins louder than (dB level of the mean linear amplitude) − 12 dB, which drops quiet bins and pauses. The mean is taken over 10^(dB/20) values, not over power.
2. The kept values are **round-tripped through `%.6g` text** before segmentation.
3. Prefix sums are accumulated sequentially in f64 per chunk. SSD = Q - S²/len.
4. Tie-breaking in the DP keeps the first (smallest) i. Comparisons are strict (`>`).
5. Output positions are `orig_bin_index * trunc(sr*0.05)` in samples, 32-bit unsigned multiply.

## 5. Uncertainties
* `operator>>(double)` in libstdc++ (gcc 4.0) cannot parse `inf`/`nan`. If calc_rms ever writes `-inf` for silent
  bins, the read in step (a) stops at that token, and all later bins are silently dropped. This depends on calc_rms's floor (see calc_rms spec).
  The same applies to `%g` output of -inf in the hand-off string, but -inf can't pass the threshold anyway.
* The progress flag at 0x165aac reads as constant 0 in the file, which gives the 35→69 % range. This has no audio effect.
* `ostream<<(unsigned long)` for positions: values ≥ 2^31 would print unsigned. Not reachable for normal lengths.
