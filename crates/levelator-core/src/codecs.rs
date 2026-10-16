//! Compressed formats: decoding through symphonia (FLAC, MP3, AAC/ALAC in
//! MP4/M4A, Ogg Vorbis) and encoding to FLAC (flacenc), MP3 (LAME) and
//! Ogg Vorbis (libvorbis/aoTuV).
//!
//! None of these existed in the original Levelator, which took only WAV and
//! AIFF.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::num::{NonZeroU32, NonZeroU8};
use std::path::Path;

use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::audio::{to_int, AudioBuffer, AudioFormat, Container, Encoding};
use crate::Error;

/// Settings for the lossy encoders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncodeOptions {
    /// MP3 constant bitrate in kbit/s (one of LAME's standard rates).
    pub mp3_kbps: u32,
    /// Ogg Vorbis quality, -0.1 ..= 1.0 (0.5 is about 160 kbit/s stereo).
    pub vorbis_quality: f32,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            mp3_kbps: 192,
            vorbis_quality: 0.5,
        }
    }
}

/// Decode any format symphonia understands into normalised interleaved doubles.
pub fn decode(path: &Path) -> Result<AudioBuffer, Error> {
    let bad = |m: String| Error::Format(format!("{}: {m}", path.display()));
    let file = File::open(path).map_err(|e| Error::open_input(path, e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let fmt_opts = FormatOptions::default();
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, fmt_opts, MetadataOptions::default())
        .map_err(|e| bad(format!("unsupported or unrecognised audio format ({e})")))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| bad("no audio track".into()))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| bad("no audio track".into()))?
        .clone();

    let (container, encoding) = match params.codec {
        CODEC_ID_FLAC => (
            Container::Flac,
            Encoding::Pcm(params.bits_per_sample.unwrap_or(16) as u16),
        ),
        CODEC_ID_ALAC => (
            Container::M4a,
            Encoding::Pcm(params.bits_per_sample.unwrap_or(16) as u16),
        ),
        CODEC_ID_MP3 => (Container::Mp3, Encoding::Float(32)),
        CODEC_ID_AAC => (Container::M4a, Encoding::Float(32)),
        CODEC_ID_VORBIS => (Container::Ogg, Encoding::Float(32)),
        CODEC_ID_OPUS => return Err(bad("Opus is not supported".into())),
        _ => return Err(bad("unsupported audio codec".into())),
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| bad(format!("cannot decode: {e}")))?;

    let mut data: Vec<f64> = Vec::new();
    let mut packet_buf: Vec<f64> = Vec::new();
    let mut spec: Option<(u32, u16)> = params
        .sample_rate
        .zip(params.channels.as_ref().map(|c| c.count() as u16));
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(bad(format!("read error: {e}"))),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                if spec.is_none() {
                    spec = Some((buf.spec().rate(), buf.spec().channels().count() as u16));
                }
                packet_buf.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut packet_buf);
                data.extend_from_slice(&packet_buf);
            }
            // Skip corrupt packets, as players do.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(bad(format!("decode error: {e}"))),
        }
    }
    let (sample_rate, channels) =
        spec.ok_or_else(|| bad("unknown sample rate or channel count".into()))?;
    if channels == 0 {
        return Err(bad("no channels".into()));
    }
    let encoding = match encoding {
        Encoding::Pcm(b) if !(8..=32).contains(&b) => Encoding::Pcm(16),
        e => e,
    };
    Ok(AudioBuffer {
        format: AudioFormat {
            container,
            encoding,
            sample_rate,
            channels,
        },
        data,
    })
}

fn create(path: &Path) -> Result<BufWriter<File>, Error> {
    File::create(path)
        .map(BufWriter::new)
        .map_err(|e| Error::Write(format!("{}: {e}", path.display())))
}

/// FLAC, at the source bit depth (24-bit for float sources, 16..=24 otherwise).
pub fn write_flac(path: &Path, buf: &AudioBuffer) -> Result<(), Error> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let werr = |m: String| Error::Write(format!("{}: {m}", path.display()));
    let bits = match buf.format.encoding {
        Encoding::Pcm(b) => b.clamp(16, 24),
        Encoding::Float(_) => 24,
    };
    let source = F64Source { buf, bits, pos: 0 };
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| werr(format!("{e:?}")))?;
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| werr(format!("{e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| werr(format!("{e:?}")))?;
    let mut bytes = sink.into_inner();
    // flacenc records the (shorter) last block as the minimum block size.
    // The spec excludes the last block, and decoders such as symphonia treat
    // min != max as a variable-blocksize stream, so make it a fixed one.
    if bytes.len() > 12 && &bytes[..4] == b"fLaC" {
        let max_block: [u8; 2] = [bytes[10], bytes[11]];
        bytes[8..10].copy_from_slice(&max_block);
    }
    let mut w = create(path)?;
    w.write_all(&bytes)
        .and_then(|_| w.flush())
        .map_err(|e| werr(e.to_string()))
}

/// Feeds an `AudioBuffer` to flacenc block by block, converting on the fly so
/// no full-size integer copy is needed.
struct F64Source<'a> {
    buf: &'a AudioBuffer,
    bits: u16,
    pos: usize,
}

impl flacenc::source::Source for F64Source<'_> {
    fn channels(&self) -> usize {
        self.buf.num_channels()
    }
    fn bits_per_sample(&self) -> usize {
        self.bits as usize
    }
    fn sample_rate(&self) -> usize {
        self.buf.format.sample_rate as usize
    }
    fn read_samples<F: flacenc::source::Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> Result<usize, flacenc::error::SourceError> {
        let ch = self.buf.num_channels();
        let end = (self.pos + block_size * ch).min(self.buf.data.len());
        let block: Vec<i32> = self.buf.data[self.pos..end]
            .iter()
            .map(|&x| to_int(x, self.bits))
            .collect();
        self.pos = end;
        dest.fill_interleaved(&block)?;
        Ok(block.len() / ch)
    }
    fn len_hint(&self) -> Option<usize> {
        Some(self.buf.frames())
    }
}

/// MP3 via LAME, constant bitrate. Mono or stereo only.
pub fn write_mp3(path: &Path, buf: &AudioBuffer, opts: &EncodeOptions) -> Result<(), Error> {
    use mp3lame_encoder::{Bitrate, Builder, FlushGap, InterleavedPcm, MonoPcm, Quality};

    let werr = |m: String| Error::Write(format!("{}: {m}", path.display()));
    let ch = buf.num_channels();
    if ch > 2 {
        return Err(werr("MP3 output supports mono or stereo only".into()));
    }
    let bitrate = match opts.mp3_kbps {
        0..=64 => Bitrate::Kbps64,
        65..=96 => Bitrate::Kbps96,
        97..=128 => Bitrate::Kbps128,
        129..=160 => Bitrate::Kbps160,
        161..=192 => Bitrate::Kbps192,
        193..=224 => Bitrate::Kbps224,
        225..=256 => Bitrate::Kbps256,
        _ => Bitrate::Kbps320,
    };
    let mut b = Builder::new().ok_or_else(|| werr("cannot start the MP3 encoder".into()))?;
    b.set_num_channels(ch as u8)
        .map_err(|e| werr(format!("{e:?}")))?;
    b.set_sample_rate(buf.format.sample_rate)
        .map_err(|e| werr(format!("{e:?}")))?;
    b.set_brate(bitrate).map_err(|e| werr(format!("{e:?}")))?;
    b.set_quality(Quality::NearBest)
        .map_err(|e| werr(format!("{e:?}")))?;
    let mut enc = b.build().map_err(|e| werr(format!("{e:?}")))?;

    // Encode into memory: the LAME/Info tag at the start of the file is only
    // known once everything is encoded.
    let mut body = Vec::new();
    let frames_per_chunk = 1152 * 32;
    for chunk in buf.data.chunks(frames_per_chunk * ch) {
        body.reserve(mp3lame_encoder::max_required_buffer_size(chunk.len() / ch));
        let res = if ch == 1 {
            enc.encode_to_vec(MonoPcm(chunk), &mut body)
        } else {
            enc.encode_to_vec(InterleavedPcm(chunk), &mut body)
        };
        res.map_err(|e| werr(format!("{e:?}")))?;
    }
    body.reserve(7200);
    // FlushGap pads the final frame instead of dropping the tail.
    enc.flush_to_vec::<FlushGap>(&mut body)
        .map_err(|e| werr(format!("{e:?}")))?;

    // LAME reserved an empty frame at the start for the LAME/Info tag; fill
    // it in so decoders can trim the encoder delay and padding (gapless).
    let tag_len = enc.lame_tag_size();
    if tag_len > 0 && tag_len <= body.len() {
        let mut tag = vec![std::mem::MaybeUninit::<u8>::uninit(); tag_len];
        if let Some(n) = enc.lame_tag_encode(&mut tag) {
            for (dst, src) in body.iter_mut().zip(&tag[..n.get()]) {
                // SAFETY: lame_tag_encode initialised the first `n` bytes.
                *dst = unsafe { src.assume_init() };
            }
        }
    }
    let mut w = create(path)?;
    w.write_all(&body)
        .and_then(|_| w.flush())
        .map_err(|e| werr(e.to_string()))
}

/// Ogg Vorbis, quality-based VBR.
pub fn write_ogg(path: &Path, buf: &AudioBuffer, opts: &EncodeOptions) -> Result<(), Error> {
    use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

    let werr = |m: String| Error::Write(format!("{}: {m}", path.display()));
    let ch = buf.num_channels();
    let rate = NonZeroU32::new(buf.format.sample_rate)
        .ok_or_else(|| werr("invalid sample rate".into()))?;
    let channels = NonZeroU8::new(ch as u8).ok_or_else(|| werr("no channels".into()))?;
    let mut builder = VorbisEncoderBuilder::new(rate, channels, create(path)?)
        .map_err(|e| werr(e.to_string()))?;
    builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr {
        target_quality: opts.vorbis_quality.clamp(-0.1, 1.0),
    });
    let mut enc = builder.build().map_err(|e| werr(e.to_string()))?;
    let mut planar = vec![Vec::new(); ch];
    for chunk in buf.data.chunks(4096 * ch) {
        for (c, p) in planar.iter_mut().enumerate() {
            p.clear();
            p.extend(chunk.iter().skip(c).step_by(ch).map(|&x| x as f32));
        }
        enc.encode_audio_block(&planar)
            .map_err(|e| werr(e.to_string()))?;
    }
    let mut w = enc.finish().map_err(|e| werr(e.to_string()))?;
    w.flush().map_err(|e| werr(e.to_string()))
}
