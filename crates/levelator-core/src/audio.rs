//! Audio file I/O for WAV and AIFF/AIFF-C.
//!
//! The original engine used libsndfile and worked on normalised `double`
//! samples. The integer <-> float scaling here follows libsndfile's
//! `sf_read_double` / `sf_write_double` with normalisation on: reading divides
//! by `2^(bits-1)` and writing multiplies by `2^(bits-1) - 1`. Samples are
//! clipped when written; libsndfile would wrap them instead.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::Error;

/// Container format of an audio file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Wav,
    Aiff,
}

/// Sample encoding of an audio file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Signed (or, for 8-bit WAV, offset-binary) integer PCM with this many bits.
    Pcm(u16),
    /// IEEE float with this many bits (32 or 64).
    Float(u16),
}

impl Encoding {
    pub fn bits(self) -> u16 {
        match self {
            Encoding::Pcm(b) | Encoding::Float(b) => b,
        }
    }
}

/// Everything needed to write a file back in the same format it was read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub container: Container,
    pub encoding: Encoding,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Decoded audio: one `Vec<f64>` per channel, normalised to roughly [-1, 1).
#[derive(Clone, Debug)]
pub struct AudioBuffer {
    pub format: AudioFormat,
    pub channels: Vec<Vec<f64>>,
}

impl AudioBuffer {
    pub fn new(format: AudioFormat, frames: usize) -> Self {
        AudioBuffer {
            format,
            channels: vec![vec![0.0; frames]; format.channels as usize],
        }
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn sample_rate(&self) -> u32 {
        self.format.sample_rate
    }

    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }
}

fn read_scale(bits: u16) -> f64 {
    1.0 / (1u64 << (bits - 1)) as f64
}

fn write_scale(bits: u16) -> f64 {
    ((1u64 << (bits - 1)) - 1) as f64
}

/// Convert a normalised double to an integer sample of `bits` bits the way
/// libsndfile 1.0.21 does with normalisation on: `lrint(x * 127)` for 8-bit,
/// `lrint(x * 32767)` for 16-bit, and for 24/32-bit `lrint(x * 0x7FFFFFFF)`
/// with 24-bit keeping the top three bytes (arithmetic shift). Results are
/// clipped to the valid range.
fn to_int(x: f64, bits: u16) -> i32 {
    let clip = |v: f64, b: u16| {
        let max = ((1i64 << (b - 1)) - 1) as f64;
        let min = -((1i64 << (b - 1)) as f64);
        v.clamp(min, max) as i32
    };
    match bits {
        24 => clip((x * 2147483647.0).round_ties_even(), 32) >> 8,
        _ => clip((x * write_scale(bits)).round_ties_even(), bits),
    }
}

/// The value a sample has after being written to and read back from a file
/// with this encoding (one temp-file round trip in the original pipeline).
pub fn quantize(x: f64, encoding: Encoding) -> f64 {
    match encoding {
        Encoding::Pcm(bits) => to_int(x, bits) as f64 * read_scale(bits),
        Encoding::Float(32) => x as f32 as f64,
        Encoding::Float(_) => x,
    }
}

/// Detect the container from the first bytes of a file.
pub fn sniff(path: &Path) -> Result<Container, Error> {
    let mut f = File::open(path).map_err(|e| Error::open_input(path, e))?;
    let mut magic = [0u8; 12];
    f.read_exact(&mut magic)
        .map_err(|_| Error::Format(format!("{}: file too short", path.display())))?;
    match (&magic[0..4], &magic[8..12]) {
        (b"RIFF", b"WAVE") | (b"RIFX", b"WAVE") => Ok(Container::Wav),
        (b"FORM", b"AIFF") | (b"FORM", b"AIFC") => Ok(Container::Aiff),
        _ => Err(Error::Format(format!(
            "{}: not a WAV or AIFF file",
            path.display()
        ))),
    }
}

/// Read a WAV or AIFF file into normalised doubles.
pub fn read(path: &Path) -> Result<AudioBuffer, Error> {
    match sniff(path)? {
        Container::Wav => read_wav(path),
        Container::Aiff => read_aiff(path),
    }
}

/// Write `buf` to `path` using `buf.format`.
pub fn write(path: &Path, buf: &AudioBuffer) -> Result<(), Error> {
    match buf.format.container {
        Container::Wav => write_wav(path, buf),
        Container::Aiff => write_aiff(path, buf),
    }
}

// ---------------------------------------------------------------- WAV

fn read_wav(path: &Path) -> Result<AudioBuffer, Error> {
    let reader = hound::WavReader::open(path).map_err(|e| Error::Format(format!("{}: {e}", path.display())))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err(Error::Format(format!("{}: no channels", path.display())));
    }
    let encoding = match spec.sample_format {
        hound::SampleFormat::Int => Encoding::Pcm(spec.bits_per_sample),
        hound::SampleFormat::Float => Encoding::Float(spec.bits_per_sample),
    };
    let format = AudioFormat {
        container: Container::Wav,
        encoding,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
    };
    let frames = reader.duration() as usize;
    let mut buf = AudioBuffer::new(format, frames);
    let wav_err = |e: hound::Error| Error::Format(format!("{}: {e}", path.display()));
    match encoding {
        Encoding::Pcm(bits) => {
            let scale = read_scale(bits);
            for (i, s) in reader.into_samples::<i32>().enumerate() {
                buf.channels[i % channels][i / channels] = s.map_err(wav_err)? as f64 * scale;
            }
        }
        Encoding::Float(_) => {
            for (i, s) in reader.into_samples::<f32>().enumerate() {
                buf.channels[i % channels][i / channels] = s.map_err(wav_err)? as f64;
            }
        }
    }
    Ok(buf)
}

fn write_wav(path: &Path, buf: &AudioBuffer) -> Result<(), Error> {
    let f = &buf.format;
    let spec = hound::WavSpec {
        channels: f.channels,
        sample_rate: f.sample_rate,
        bits_per_sample: f.encoding.bits(),
        sample_format: match f.encoding {
            Encoding::Pcm(_) => hound::SampleFormat::Int,
            Encoding::Float(_) => hound::SampleFormat::Float,
        },
    };
    let wav_err = |e: hound::Error| Error::Write(format!("{}: {e}", path.display()));
    let mut w = hound::WavWriter::create(path, spec).map_err(wav_err)?;
    for i in 0..buf.frames() {
        for ch in &buf.channels {
            match f.encoding {
                Encoding::Pcm(bits) => w.write_sample(to_int(ch[i], bits)).map_err(wav_err)?,
                Encoding::Float(_) => w.write_sample(ch[i] as f32).map_err(wav_err)?,
            }
        }
    }
    w.finalize().map_err(wav_err)
}

// ---------------------------------------------------------------- AIFF

fn read_u16be(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn read_u32be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Decode an 80-bit IEEE 754 extended big-endian float (AIFF sample rate).
fn ext80_to_f64(b: &[u8]) -> f64 {
    let exp = (((b[0] & 0x7f) as i32) << 8) | b[1] as i32;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    let v = mant as f64 * 2f64.powi(exp - 16383 - 63);
    if b[0] & 0x80 != 0 { -v } else { v }
}

fn f64_to_ext80(v: f64) -> [u8; 10] {
    let mut out = [0u8; 10];
    if v == 0.0 {
        return out;
    }
    let sign = if v < 0.0 { 0x80u8 } else { 0 };
    let v = v.abs();
    let mut exp = v.log2().floor() as i32;
    let mut mant = v / 2f64.powi(exp);
    if mant >= 2.0 {
        mant /= 2.0;
        exp += 1;
    }
    let e = (exp + 16383) as u16;
    let m = (mant * 2f64.powi(63)) as u64;
    out[0] = sign | ((e >> 8) as u8 & 0x7f);
    out[1] = e as u8;
    out[2..10].copy_from_slice(&m.to_be_bytes());
    out
}

fn read_aiff(path: &Path) -> Result<AudioBuffer, Error> {
    let bad = |m: &str| Error::Format(format!("{}: {m}", path.display()));
    let mut r = BufReader::new(File::open(path).map_err(|e| Error::open_input(path, e))?);
    let mut hdr = [0u8; 12];
    r.read_exact(&mut hdr).map_err(|_| bad("truncated header"))?;
    let is_aifc = &hdr[8..12] == b"AIFC";

    let mut comm: Option<(u16, u32, u16, f64, [u8; 4])> = None;
    let mut ssnd: Option<(u64, u64)> = None;
    loop {
        let mut ck = [0u8; 8];
        if r.read_exact(&mut ck).is_err() {
            break;
        }
        let size = read_u32be(&ck[4..8]) as u64;
        let pos = r.stream_position().map_err(|_| bad("seek failed"))?;
        match &ck[0..4] {
            b"COMM" => {
                let mut c = vec![0u8; size as usize];
                r.read_exact(&mut c).map_err(|_| bad("truncated COMM chunk"))?;
                if c.len() < 18 {
                    return Err(bad("short COMM chunk"));
                }
                let mut comp = *b"NONE";
                if is_aifc && c.len() >= 22 {
                    comp.copy_from_slice(&c[18..22]);
                }
                comm = Some((
                    read_u16be(&c[0..2]),
                    read_u32be(&c[2..6]),
                    read_u16be(&c[6..8]),
                    ext80_to_f64(&c[8..18]),
                    comp,
                ));
            }
            b"SSND" => {
                let mut o = [0u8; 8];
                r.read_exact(&mut o).map_err(|_| bad("truncated SSND chunk"))?;
                let offset = read_u32be(&o[0..4]) as u64;
                ssnd = Some((pos + 8 + offset, size.saturating_sub(8 + offset)));
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        r.seek(SeekFrom::Start(pos + size + (size & 1)))
            .map_err(|_| bad("seek failed"))?;
    }
    let (channels, frames, bits, rate, comp) = comm.ok_or_else(|| bad("missing COMM chunk"))?;
    if channels == 0 {
        return Err(bad("no channels"));
    }
    let (encoding, little) = match &comp {
        b"NONE" | b"twos" => (Encoding::Pcm(bits), false),
        b"sowt" => (Encoding::Pcm(bits), true),
        b"fl32" | b"FL32" => (Encoding::Float(32), false),
        b"fl64" | b"FL64" => (Encoding::Float(64), false),
        _ => return Err(bad("unsupported AIFF-C compression (only uncompressed PCM/float is supported)")),
    };
    let width = (encoding.bits() as usize).div_ceil(8);
    if !(1..=8).contains(&width) {
        return Err(bad("unsupported sample size"));
    }
    let format = AudioFormat {
        container: Container::Aiff,
        encoding,
        sample_rate: rate.round() as u32,
        channels,
    };
    let mut buf = AudioBuffer::new(format, frames as usize);
    if let Some((start, len)) = ssnd {
        r.seek(SeekFrom::Start(start)).map_err(|_| bad("seek failed"))?;
        let frame_bytes = width * channels as usize;
        let n = (frames as u64).min(len / frame_bytes as u64) as usize;
        let mut data = vec![0u8; n * frame_bytes];
        r.read_exact(&mut data).map_err(|_| bad("truncated sample data"))?;
        let scale = read_scale(width as u16 * 8);
        for (i, s) in data.chunks_exact(width).enumerate() {
            let mut bytes = s.to_vec();
            if little {
                bytes.reverse();
            }
            let v = match encoding {
                Encoding::Float(32) => f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64,
                Encoding::Float(_) => f64::from_be_bytes(bytes[..8].try_into().unwrap()),
                Encoding::Pcm(_) => {
                    // Sign-extend a big-endian integer of `width` bytes.
                    let mut raw: i64 = if bytes[0] & 0x80 != 0 { -1 } else { 0 };
                    for b in &bytes {
                        raw = (raw << 8) | *b as i64;
                    }
                    raw as f64 * scale
                }
            };
            buf.channels[i % channels as usize][i / channels as usize] = v;
        }
        buf.channels.iter_mut().for_each(|c| c.truncate(n));
    } else {
        buf.channels.iter_mut().for_each(Vec::clear);
    }
    Ok(buf)
}

fn write_aiff(path: &Path, buf: &AudioBuffer) -> Result<(), Error> {
    let werr = |e: std::io::Error| Error::Write(format!("{}: {e}", path.display()));
    let f = &buf.format;
    let frames = buf.frames();
    let (aifc, comp, bits): (bool, &[u8; 4], u16) = match f.encoding {
        Encoding::Pcm(b) => (false, b"NONE", b),
        Encoding::Float(32) => (true, b"fl32", 32),
        Encoding::Float(_) => (true, b"fl64", 64),
    };
    let width = (bits as usize).div_ceil(8);
    let data_len = frames * width * f.channels as usize;
    let comm_len: u32 = if aifc { 18 + 4 + 2 } else { 18 };
    let ssnd_len = 8 + data_len as u32;
    let fver = if aifc { 12 } else { 0 };
    let form_len = 4 + fver + 8 + comm_len + 8 + ssnd_len + (ssnd_len & 1);

    let mut w = BufWriter::new(File::create(path).map_err(werr)?);
    w.write_all(b"FORM").map_err(werr)?;
    w.write_all(&form_len.to_be_bytes()).map_err(werr)?;
    w.write_all(if aifc { b"AIFC" } else { b"AIFF" }).map_err(werr)?;
    if aifc {
        w.write_all(b"FVER").map_err(werr)?;
        w.write_all(&4u32.to_be_bytes()).map_err(werr)?;
        w.write_all(&0xA280_5140u32.to_be_bytes()).map_err(werr)?;
    }
    w.write_all(b"COMM").map_err(werr)?;
    w.write_all(&comm_len.to_be_bytes()).map_err(werr)?;
    w.write_all(&f.channels.to_be_bytes()).map_err(werr)?;
    w.write_all(&(frames as u32).to_be_bytes()).map_err(werr)?;
    w.write_all(&bits.to_be_bytes()).map_err(werr)?;
    w.write_all(&f64_to_ext80(f.sample_rate as f64)).map_err(werr)?;
    if aifc {
        w.write_all(comp).map_err(werr)?;
        w.write_all(&[0, 0]).map_err(werr)?; // empty pascal string, padded
    }
    w.write_all(b"SSND").map_err(werr)?;
    w.write_all(&ssnd_len.to_be_bytes()).map_err(werr)?;
    w.write_all(&[0u8; 8]).map_err(werr)?;
    for i in 0..frames {
        for ch in &buf.channels {
            match f.encoding {
                Encoding::Float(32) => w.write_all(&(ch[i] as f32).to_be_bytes()),
                Encoding::Float(_) => w.write_all(&ch[i].to_be_bytes()),
                Encoding::Pcm(b) => {
                    let v = to_int(ch[i], b) as i64;
                    w.write_all(&v.to_be_bytes()[8 - width..])
                }
            }
            .map_err(werr)?;
        }
    }
    if ssnd_len & 1 == 1 {
        w.write_all(&[0]).map_err(werr)?;
    }
    w.flush().map_err(werr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext80_roundtrip() {
        for r in [8000.0, 22050.0, 44100.0, 48000.0, 96000.0] {
            assert_eq!(ext80_to_f64(&f64_to_ext80(r)), r);
        }
    }

    #[test]
    fn int_conversion_matches_libsndfile() {
        assert_eq!(to_int(1.0, 16), 32767);
        assert_eq!(to_int(-1.0, 16), -32767);
        assert_eq!(to_int(-2.0, 16), -32768);
        assert_eq!(to_int(0.5, 16), 16384); // 16383.5 rounds to even
        assert_eq!(read_scale(16), 1.0 / 32768.0);
        assert_eq!(to_int(1.0, 24), 8388607);
        assert_eq!(to_int(-1.0, 24), -8388608);
        assert_eq!(to_int(0.25, 8), 32);
    }

    fn roundtrip(container: Container, encoding: Encoding) {
        let dir = std::env::temp_dir().join(format!("levelator-audio-{container:?}-{encoding:?}"));
        let format = AudioFormat { container, encoding, sample_rate: 44100, channels: 2 };
        let mut buf = AudioBuffer::new(format, 100);
        for i in 0..100 {
            buf.channels[0][i] = (i as f64 / 100.0) - 0.5;
            buf.channels[1][i] = -buf.channels[0][i] * 0.5;
        }
        write(&dir, &buf).unwrap();
        let back = read(&dir).unwrap();
        std::fs::remove_file(&dir).ok();
        assert_eq!(back.format, format);
        assert_eq!(back.frames(), 100);
        let tol = match encoding {
            Encoding::Pcm(b) => 2.0 / (1u64 << (b - 1)) as f64,
            Encoding::Float(_) => 1e-6,
        };
        for c in 0..2 {
            for i in 0..100 {
                assert!((back.channels[c][i] - buf.channels[c][i]).abs() <= tol);
            }
        }
    }

    #[test]
    fn wav_roundtrips() {
        roundtrip(Container::Wav, Encoding::Pcm(16));
        roundtrip(Container::Wav, Encoding::Pcm(24));
        roundtrip(Container::Wav, Encoding::Float(32));
    }

    #[test]
    fn aiff_roundtrips() {
        roundtrip(Container::Aiff, Encoding::Pcm(16));
        roundtrip(Container::Aiff, Encoding::Pcm(24));
        roundtrip(Container::Aiff, Encoding::Float(32));
    }
}
