# Orchestrator — FUN_00028dfc (`level.cpp`) and main

Evidence: re/fn/028dfc.c, asm 0x28dfc–0x2cc6b; helpers 0x28a9c (constant-signal test), 0x28d04 (copy file),
0x28c7c (elapsed time), 0x5b166 (progress), 0x5add6 (error print), 0x58296 (command line), CCleaner 0x2d600/0x2e9c4/0x2f64e.

## Entry
`main(argc, argv)` (0x2f5a) → `FUN_00028dfc(argc, argv, 0, 0, 0)`; its return value is the **process exit code**
(0 success, 1 failure). If an exception escapes, main's handler (type index 1) prints `e.what()` (virtual slot +8) to
**stderr** + newline and returns **1**; other exception types are re-thrown (terminate).
Args 4/5 are a progress callback and its user data (stored in 0x1f1398 / 0x1f1394) — both NULL ⇒ progress is printed
to stdout.

## Compile-time switches (int constants in __literal4, read through pointer slots)
| slot → const | value | meaning when non-zero |
|---|---|---|
| 0x1f33ec → 0x165aa0 | **0** | debug: keep temp files, print `"elapsed time: %4.1f seconds"` (clock()/100), extra options |
| 0x1f3788 → 0x165aa4 | **1** | 3rd positional `key` required and checked |
| 0x1f339c → 0x165aa8 | **0** | "log" mode: EncryptedLog + phone-home upload (see below) |
| 0x1f3374 → 0x165aac | **0** | skip steps 1–4 (process the input directly) |
| 0x1f33bc → 0x1a7010 (__data) | runtime 0 | cancel flag; checked before every step → throw `ss_cancel_exception` |
All 0 except key-check ⇒ the port only needs the normal path.

## Command line (FUN_00058296), release build
- `bit-depth,b` int, default **0** ("bit depth of output") — only used to build normalize's `-b` arg.
- `help,h`, `options-file,o`, `version` ("1.0." "2196" + libsndfile version, then throws).
- hidden positionals: `input-file`, `output-file`, `key`.
- Missing input/output → stderr `"Usage: level input_file output_file"`, return 1.
- Key check: `s = "The Levelator copyright (c) 2006 by GigaVox Media, Inc. and Singular Software" + sprintf("%lu", t)`,
  `t = (time(NULL)/10)*10`; key must equal lowercase-hex MD5(s); if not, retried with `t-10`; failure → stderr `"error"`,
  return 1. (GUI computes the same; a port may drop or replicate this.)

## Control flow (normal build)

```
cleaner = CCleaner{}                                   // vector<CTempFile{path, marked=false}>
if cancel { throw }
parse args → (bit_depth, in, out, key); on failure return 1
if !exists(in) { stderr "Could not find <in>"; return 1 }
snd = SoundFile(in, read, norm=true)                    // FUN_0000408e; failure → runtime_error "error opening sound file \"<in>\"" (exit 1)
info = snd.info
duration = (double)info.frames / (double)info.samplerate
if cancel { throw }
if !(duration >= 0.05) {                               // ucomisd dur, 0.05 (f64 0x1a6f68); NaN goes to the >= branch
    progress(100,100,frac=1.0)  → prints "\r100%"
    return copy_file(in, out)                          // 0x28d04: if exists(out) remove(out); copy → returns 0
}
if is_constant(snd, info) {                            // 0x28a9c
    progress 100%; return copy_file(in, out)
}
if duration < 0.0 { … normalize-only path … }          // DEAD CODE (frames, rate ≥ 0): normalize -l -18.0 -b %d in out,
                                                       // "error in normalization step" on failure
close snd
dot = in.rfind('.'); if none { stderr "Could not find extension for input sound file"; return 1 }
avail = boost::filesystem::space("/").available
if avail < 3 * file_size(in) { throw runtime_error("Insufficient disk space for temporary files.") }  // u64 compare
base = if exists("/") { "/" + &tmpnam(NULL)[1..] (strip one trailing '.') } else { in[..dot] }   // "/" always exists
ext  = in[dot..]                                       // includes the '.', e.g. ".wav"
```
Temp files (each registered with `cleaner.add` before the step that creates it):

| # | name | produced by |
|---|---|---|
| 1 | base+"_01"+ext | highpass |
| 2 | base+"_02"+ext | gate |
| 3 | base+"_03"+ext | reduce_loud_peak |
| 4 | base+"_04"+ext | reduce_loud |
| 5 | base+"_05.txt" | calc_rms |
| 6 | base+"_06.clt" | wrap_segment |
| 7 | base+"_07.txt" | calculate_gains `--rms-file` |
| 8 | base+"_08.txt" | calculate_gains `--gains-file` |
| 9 | base+"_09.txt" | smooth_gains |
| 10 | base+"_10"+ext | apply gains |

### Steps (argv exactly as built; values of `*0x1f…` option strings are listed in the brief / stage specs)

1. `highpass(9, {"highpass","--highpass__low","25.0","--highpass__high","100.0","--highpass__verbosity","0", in, _01})`
   — returns a **double**; success iff `ret < 0.0` (ucomisd ret,0.0; jb ok). Else "Step 1 did not complete successfully".
   The returned value (an input level in dB — see highpass.md) gives the gate threshold:
   `thr = minsd(-40.0, ret - 18.0)` i.e. `min(ret-18, -40)`, formatted `sprintf("%.1lf", thr)`.
2. `gate(23, {"gate","--gate__attenuation_max","-12.0","--gate__attenuation_min","0.0","--gate__threshold_attack",thr,
   "--gate__threshold_release",thr,"--gate__attack","250","--gate__release","750","--gate__hold_time",*0x1f33fc,
   "--gate__buffer_size","5","--gate__anticipate_attack",*0x1f3428,"--gate__verbosity",*0x1f342c, _01, _02})` → 0 = OK.
   Then delete _01.
3. `reduce_loud_peak(15, {…,"--level_diff","15.0","--attack",…,"--release",…,"--noise-floor",…,"--bin-size",…,
   "--verbosity",…, _02, _03})`; delete _02.
4. `reduce_loud(13, {…,"--level_diff","3.0","--attack",*0x1f33cc,"--release","150","--noise-floor",*0x1f33d8,
   "--bin-size","50", _03, _04})`; delete _03.
5. `calc_rms(3, {"calc_rms", _04, _05.txt})`.
6. `wrap_segment(_05.txt, "whereisthisused", "..\\options_segment.txt", _06.clt, -12.0, samplerate, 0.05)`
   (FUN_00055774; see segment.md); delete _05.txt.
7. `calculate_gains(13, {…,"--noise-threshold","0.006","--bin-size","50","--gains-file",_08.txt,"--rms-file",_07.txt,
   "--gain-limit","24.0", _04, _06.clt})`; delete _06.clt.
8. `smooth_gains(7, {"smooth_gains","-a","40","-b","2", _08.txt, _09.txt})`; delete _07.txt and _08.txt.
9. globals: bin_ms=50, gains=_09.txt, in=_04, out=_10 → `apply_gains()` (void, result ignored); delete _04 and _09.txt.
   `if !exists(_10) { stderr "Could not find <_10>"; return 1 }`
10. `normalize(7, {"normalize","-l","-18.0","-b",sprintf("%d",bit_depth), _10, out})`; non-zero →
    "Step 9 did not complete successfully" (sic); success → delete _10, then `cout << endl`, return 0.

Every step: before it, `if cancel { throw ss_cancel_exception }`; on non-zero return: stderr `"Step N did not complete
successfully"` and return 1. "Deleting" = `markForDeletion(path)` + `clean()` (boost::filesystem::remove), only when
the debug switch is 0. **On any failure/exception the already-created temp files are NOT removed** (CTempFile has no
deleting destructor, entries default to marked=false).

## Constant-signal test (0x28a9c)
```
first = readf(1 frame); seek(0)
for blk in 0..ceil(frames/8192):            // 64-bit counter
    k = readf(buf, 8192)
    for j in 0..k, c in 0..channels:  if !(buf[j*ch+c] == first[c]) → return false   // NaN ⇒ false
return true                                  // ⇒ copy input → output unchanged, progress 100%
```
(The check is done per block; any mismatch returns false immediately after that block.)

## Progress (FUN_0005b166)
Struct `{f64 start, f64 end, i32 last=-1, i32 cur=-1, cb, ud}`; `report(frac)`:
```
if cancel { throw }
v = floor(start + (end - start) * frac) as i32      // SSE2 double, cvttsd2si
cur = v
if v > last { last = v; if cb { cb(v as f32 / 100.0, ud) } else { cout << "\r" << v << "%" << flush } }
```
Each stage creates its own struct (last=-1), so the first report of a stage always prints (stage boundaries may
repeat a number). Ranges in the normal build (call sites):

| stage | range | site |
|---|---|---|
| highpass | 0 → 14 | 0x3dc04 |
| gate (two passes) | 14 → 20 | 0x3b137, 0x3b856 |
| reduce_loud_peak | 20 → 24, 24 → 27 | 0x49ff0, 0x49b0a |
| reduce_loud | 27 → 30, 30 → 34 | 0x46266, 0x45d80 |
| calc_rms | 34 → 35 | 0x32e17 |
| segment | 35 → 69 | 0x4e835 |
| calculate_gains | 69 → 82 | 0x355a7 |
| apply gains | 82 → 91 | 0x2fccd |
| normalize analysis / write | 91 → 95, 95 → 100 | 0x40d50, 0x4152e |
| short/constant input, gate copy path | 100 → 100 | 0x29bbe, 0x29d09, 0x39b51 |

The GUI (dec/) parses these "NN%" tokens from stdout. Final line ends with `endl` after the last "\r100%"
(the short/constant copy paths do **not** print the newline).

## stderr
FUN_0005add6(file, line, msg) prints **only `msg` + '\n'** to `std::cerr` (file/line unused). Messages:
"Could not find <path>", "Could not find extension for input sound file", "Step N did not complete successfully"
(N = 1..9; normalize failure also reports "Step 9"), usage/"error" from the CLI, stage-specific errors, warnings.

## Logging / phone-home (compiled out: switch 0x165aa8 = 0) — do NOT port
When enabled: creates `$HOME/log/level/`, writes `level_<sec>_<usec 6 digits>.dat` via `EncryptedLog`
(Crypto++ RSA/DLIES) containing `<levelLog version='1.0'>` with libsndfile version, time, frames, samplerate,
channels, format (hex), input/output file names; then `system("ping -c 1 ftp.singularsoftware.com …")` and
`system("wput -R -a <dir>/wput.log --basename=<dir>/ <file> ftp://level01:…@ftp.singularsoftware.com")`, printing
wput's exit-code meaning and "Program stopped" / "Singular Software FTP site is not reachable".

## Port notes
- Temp naming/location is irrelevant to output; keep intermediates in memory if desired, **but** every audio
  intermediate (_01…_04, _10) is written in the input's own format (stages copy SF_INFO), i.e. quantised to the input
  subtype through libsndfile 1.0.21 conversions (PCM16: write `lrint(x*32767)`, read `s/32768`). A faithful port must
  emulate that round trip at each audio stage boundary.
- Text intermediates: _08 gains are 6-significant-digit `%g` text (see calculate_gains.md); _09 round-trips exactly.
