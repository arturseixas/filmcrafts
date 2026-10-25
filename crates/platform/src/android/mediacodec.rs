//! Hardware H.264 and HEVC decoding through Android's MediaCodec (the NDK's `AMediaCodec`), behind
//! [`crate::HybridDecoder`] like the other platforms' decoders.
//!
//! The codec renders into an `AImageReader` surface of `YUV_420_888` images: unlike MediaCodec's
//! byte-buffer output, whose layout is vendor specific (planar, semi-planar, aligned Qualcomm
//! variants…), an image describes every plane with its row and pixel strides, so one copy routine
//! reads them all. Each rendered picture is acquired before the next one is rendered, so a
//! picture's presentation time is the one MediaCodec reported for it.
//!
//! Only what the hardware path reproduces exactly is taken: 8-bit 4:2:0 progressive streams on a
//! hardware codec (Android's own software codecs are no faster than FilmCraft's). Everything else
//! is declined, and the factory leaves it to the software decoders.
//!
//! MediaCodec cannot continue after an end-of-stream drain, which is how [`VideoDecoder::flush`]
//! gets the pictures held for reordering. The samples since the last restart point are kept, so a
//! sample fed after a flush first replays them into the flushed codec (pictures already returned are
//! dropped); past that log's bound the next sample fails and the hybrid continues in software.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use filmcraft_codecs::hw::{NalCodec, NalStreamInfo};
use filmcraft_codecs::{CodecError, DecodedFrame, Result, VideoDecoder};
use filmcraft_frame::{Chroma, PixelData, VideoFrame, pool};
use ndk::media::image_reader::{AcquireResult, Image, ImageFormat, ImageReader};
use ndk::media::media_codec::{DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection};
use ndk::media::media_format::MediaFormat;

/// `MediaCodec.BUFFER_FLAG_END_OF_STREAM`.
const END_OF_STREAM: u32 = 4;
/// MediaCodec times are unsigned microseconds; FilmCraft's pts can be negative (edit lists).
const PTS_OFFSET: i64 = 1 << 40;
/// Hardware decoder sessions open at once. Devices have a handful (and share them with the camera
/// and other apps); thumbnails and further clips beyond this decode in software.
const MAX_SESSIONS: usize = 6;
/// Samples and bytes kept for the replay after a flush.
const LOG_MAX_SAMPLES: usize = 600;
const LOG_MAX_BYTES: usize = 128 << 20;
/// Presentation times remembered as returned since the last reset.
const EMITTED_MAX: usize = 4096;
/// How long to wait for a free input buffer, for a rendered picture, and for the end of stream.
const INPUT_WAIT: Duration = Duration::from_secs(2);
const IMAGE_WAIT: Duration = Duration::from_millis(500);
const DRAIN_WAIT: Duration = Duration::from_secs(3);

static SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// One of the [`MAX_SESSIONS`] hardware sessions, given back when dropped.
struct Slot;

impl Slot {
    fn take() -> std::result::Result<Slot, String> {
        let mut n = SESSIONS.load(Ordering::Relaxed);
        loop {
            if n >= MAX_SESSIONS {
                return Err(format!("{MAX_SESSIONS} hardware decoders are already open"));
            }
            match SESSIONS.compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Ok(Slot),
                Err(now) => n = now,
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        SESSIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A started codec rendering into its image reader. Field order is drop order: the codec stops
/// before the surface it renders into goes away.
struct Session {
    codec: MediaCodec,
    reader: ImageReader,
    _slot: Slot,
}

// SAFETY: AMediaCodec and AImageReader may be used from any thread as long as calls are not made
// concurrently; a Session is owned by one decoder, which the frame server uses from one thread at
// a time (`VideoDecoder` methods take `&mut self`). No image listener (a callback on another
// thread) is installed.
#[allow(unsafe_code)]
unsafe impl Send for Session {}

fn mime(codec: NalCodec) -> &'static str {
    match codec {
        NalCodec::H264 => "video/avc",
        NalCodec::Hevc => "video/hevc",
    }
}

/// Codec names of Android's software decoders (no faster than ours, and not hardware).
fn is_software_codec(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("omx.google.") || n.starts_with("c2.android.") || n.contains(".sw.") || n.ends_with(".sw")
}

/// Parameter sets as Annex B codec-specific data: H.264 `csd-0` (SPS) and `csd-1` (PPS); HEVC all
/// in `csd-0`.
fn codec_specific_data(info: &NalStreamInfo) -> (Vec<u8>, Vec<u8>) {
    let (mut csd0, mut csd1) = (Vec::new(), Vec::new());
    for p in &info.parameter_sets {
        let pps = info.codec == NalCodec::H264 && p.first().is_some_and(|&h| info.nal_type(h) == 8);
        let out = if pps { &mut csd1 } else { &mut csd0 };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(p);
    }
    (csd0, csd1)
}

impl Session {
    fn new(info: &NalStreamInfo) -> std::result::Result<(Session, String), String> {
        let slot = Slot::take()?;
        let mime = mime(info.codec);
        let codec = MediaCodec::from_decoder_type(mime).ok_or_else(|| format!("no {mime} decoder"))?;
        let name = codec.name().unwrap_or_else(|_| "MediaCodec".into());
        if is_software_codec(&name) {
            return Err(format!("{name} is a software decoder"));
        }
        let (w, h) = info.coded;
        let (w, h) = (i32::try_from(w).map_err(|_| "picture too wide")?, i32::try_from(h).map_err(|_| "picture too tall")?);
        let mut format = MediaFormat::new();
        format.set_str("mime", mime);
        format.set_i32("width", w);
        format.set_i32("height", h);
        let (csd0, csd1) = codec_specific_data(info);
        if !csd0.is_empty() {
            format.set_buffer("csd-0", &csd0);
        }
        if !csd1.is_empty() {
            format.set_buffer("csd-1", &csd1);
        }
        // realtime priority: the decoder feeds monitors during playback
        format.set_i32("priority", 0);
        let reader = ImageReader::new(w, h, ImageFormat::YUV_420_888, 3).map_err(|e| format!("image reader: {e:?}"))?;
        let window = reader.window().map_err(|e| format!("image reader surface: {e:?}"))?;
        codec.configure(&format, Some(&window), MediaCodecDirection::Decoder).map_err(|e| format!("{name}: configure: {e:?}"))?;
        codec.start().map_err(|e| format!("{name}: start: {e:?}"))?;
        Ok((Session { codec, reader, _slot: slot }, name))
    }

    /// The image of the picture the codec just rendered with presentation time `time_us`. A
    /// picture rendered before a flush can still reach the reader afterwards: when this device
    /// stamps images with their presentation time (`stamped`, learned from the session's first
    /// image), images with another time are dropped.
    fn next_image(&self, time_us: u64, stamped: &mut Option<bool>) -> std::result::Result<Image, String> {
        let started = Instant::now();
        let want = i64::try_from(time_us).ok().and_then(|t| t.checked_mul(1000));
        loop {
            match self.reader.acquire_next_image().map_err(|e| format!("acquire image: {e:?}"))? {
                AcquireResult::Image(img) => {
                    let matches = want.is_some() && img.timestamp().ok() == want;
                    match *stamped {
                        None => {
                            *stamped = Some(matches);
                            return Ok(img);
                        }
                        Some(true) if !matches => drop(img),
                        _ => return Ok(img),
                    }
                }
                AcquireResult::MaxImagesAcquired => return Err("image reader full".into()),
                AcquireResult::NoBufferAvailable => {}
            }
            if started.elapsed() > IMAGE_WAIT {
                return Err("a rendered picture never reached the image reader".into());
            }
            std::thread::sleep(Duration::from_micros(300));
        }
    }

    /// Drop images still waiting in the reader (after a flush).
    fn discard_images(&self) {
        while let Ok(AcquireResult::Image(img)) = self.reader.acquire_next_image() {
            drop(img);
        }
    }
}

/// A MediaCodec hardware decoder for one H.264 / HEVC stream.
pub struct McDecoder {
    info: NalStreamInfo,
    name: String,
    session: Option<Session>,
    /// The next sample starts a run (after creation or a reset): parameter sets go in front of it.
    first: bool,
    /// HEVC: RASL pictures of the CRA that started this run are skipped (they reference pictures
    /// before it), as the software decoder does.
    skip_rasl: bool,
    /// Samples since the last restart point (for the replay after a flush); `None` past the bound.
    log: Option<Vec<(Vec<u8>, i64)>>,
    log_bytes: usize,
    last_irap: usize,
    /// The codec drained to the end of stream (a flush): the next sample replays the log first.
    ended: bool,
    /// Presentation times returned since the last reset.
    emitted: BTreeSet<i64>,
    out: Vec<DecodedFrame>,
    /// Whether this device stamps each image with its picture's presentation time (unknown until
    /// the first image of a session).
    stamped: Option<bool>,
}

impl McDecoder {
    /// Open a hardware session for `info`; errors when the stream is one this path does not take
    /// or the device has no hardware decoder for it (profile, level, size, no free session).
    pub fn new(info: NalStreamInfo) -> std::result::Result<McDecoder, String> {
        if info.chroma_format_idc != 1 || info.bit_depth_luma != 8 || info.bit_depth_chroma != 8 {
            return Err(format!("{}-bit chroma format {} (8-bit 4:2:0 only)", info.bit_depth_luma, info.chroma_format_idc));
        }
        if info.interlaced {
            return Err("interlaced (field-coded) stream".into());
        }
        let (_, _, w, h) = info.crop;
        if w == 0 || h == 0 || info.coded.0 < w || info.coded.1 < h {
            return Err(format!("bad picture size {w}x{h}"));
        }
        let (session, name) = Session::new(&info)?;
        Ok(McDecoder {
            info,
            name: format!("MediaCodec ({name})"),
            session: Some(session),
            first: true,
            skip_rasl: false,
            log: Some(Vec::new()),
            log_bytes: 0,
            last_irap: 0,
            ended: false,
            emitted: BTreeSet::new(),
            out: Vec::new(),
            stamped: None,
        })
    }

    fn session(&mut self) -> std::result::Result<&Session, String> {
        if self.session.is_none() {
            self.session = Some(Session::new(&self.info)?.0);
            self.first = true;
            self.stamped = None;
        }
        self.session.as_ref().ok_or_else(|| "no MediaCodec session".to_string())
    }

    fn remember(&mut self, sample: &[u8], pts: i64) {
        if self.info.is_irap(sample) {
            let cra = self.info.codec == NalCodec::Hevc && self.info.nal_types(sample).contains(&21);
            match self.log.as_mut() {
                Some(log) if cra => {
                    log.drain(..self.last_irap.min(log.len()));
                    self.log_bytes = log.iter().map(|s| s.0.len()).sum();
                }
                _ => {
                    self.log = Some(Vec::new());
                    self.log_bytes = 0;
                }
            }
            self.last_irap = self.log.as_ref().map_or(0, Vec::len);
        }
        let Some(log) = self.log.as_mut() else { return };
        self.log_bytes = self.log_bytes.saturating_add(sample.len());
        if log.len() >= LOG_MAX_SAMPLES || self.log_bytes > LOG_MAX_BYTES {
            self.log = None;
            return;
        }
        log.push((sample.to_vec(), pts));
    }

    /// HEVC: whether `sample` is a RASL picture of the CRA that started the run (not decoded).
    fn skips(&mut self, sample: &[u8]) -> bool {
        if self.info.codec != NalCodec::Hevc {
            return false;
        }
        let types = self.info.nal_types(sample);
        if types.iter().any(|t| (16..=23).contains(t)) {
            self.skip_rasl = self.first && types.iter().any(|t| (16..=21).contains(t) && !(19..=20).contains(t));
            false
        } else {
            self.skip_rasl && types.iter().any(|t| matches!(t, 8 | 9))
        }
    }

    /// Queue one Annex B access unit (or the end of stream with `None`). While the codec has no
    /// free input buffer its finished pictures are collected: a codec whose output queue is full
    /// takes no input until they are.
    fn queue(&mut self, data: Option<&[u8]>, time_us: u64) -> std::result::Result<(), String> {
        let started = Instant::now();
        loop {
            let session = self.session()?;
            match session.codec.dequeue_input_buffer(Duration::from_millis(5)).map_err(|e| format!("input buffer: {e:?}"))? {
                DequeuedInputBufferResult::Buffer(mut buf) => {
                    let (len, flags) = match data {
                        Some(d) => {
                            let dst = buf.buffer_mut();
                            if d.len() > dst.len() {
                                return Err(format!("a {} byte sample does not fit the decoder's {} byte input buffer", d.len(), dst.len()));
                            }
                            for (o, &b) in dst.iter_mut().zip(d) {
                                o.write(b);
                            }
                            (d.len(), 0)
                        }
                        None => (0, END_OF_STREAM),
                    };
                    return session.codec.queue_input_buffer(buf, 0, len, time_us, flags).map_err(|e| format!("queue input: {e:?}"));
                }
                DequeuedInputBufferResult::TryAgainLater => {}
            }
            if started.elapsed() > INPUT_WAIT {
                return Err("the decoder takes no more input".into());
            }
            self.drain(false)?;
        }
    }

    fn feed(&mut self, sample: &[u8], pts: i64) -> std::result::Result<(), String> {
        let time = pts.checked_add(PTS_OFFSET).and_then(|t| u64::try_from(t).ok()).ok_or_else(|| format!("presentation time {pts} out of range"))?;
        let data = crate::annexb::to_annex_b(&self.info, sample, self.first)?;
        self.first = false;
        self.queue(Some(&data), time)?;
        self.drain(false)
    }

    /// Collect the pictures the codec has finished; with `to_end`, until the end of stream.
    fn drain(&mut self, to_end: bool) -> std::result::Result<(), String> {
        let started = Instant::now();
        loop {
            let Some(session) = self.session.as_ref() else { return Ok(()) };
            let wait = if to_end { Duration::from_millis(20) } else { Duration::ZERO };
            match session.codec.dequeue_output_buffer(wait).map_err(|e| format!("output buffer: {e:?}"))? {
                DequeuedOutputBufferInfoResult::Buffer(buf) => {
                    let info = *buf.info();
                    let eos = info.flags() & END_OF_STREAM != 0;
                    // the empty end-of-stream buffer carries no picture
                    let picture = !(eos && info.size() <= 0);
                    session.codec.release_output_buffer(buf, picture).map_err(|e| format!("release output: {e:?}"))?;
                    if picture {
                        let time_us = u64::try_from(info.presentation_time_us()).unwrap_or(0);
                        let img = session.next_image(time_us, &mut self.stamped)?;
                        let pts = info.presentation_time_us().saturating_sub(PTS_OFFSET);
                        let frame = to_frame(&img, &self.info)?;
                        drop(img);
                        if self.emitted.insert(pts) {
                            self.out.push(DecodedFrame { pts, frame, draft: false });
                        }
                        while self.emitted.len() > EMITTED_MAX {
                            self.emitted.pop_first();
                        }
                    }
                    if eos {
                        return Ok(());
                    }
                }
                DequeuedOutputBufferInfoResult::OutputFormatChanged | DequeuedOutputBufferInfoResult::OutputBuffersChanged => {}
                DequeuedOutputBufferInfoResult::TryAgainLater => {
                    if !to_end {
                        return Ok(());
                    }
                    if started.elapsed() > DRAIN_WAIT {
                        return Err("the decoder did not finish the stream".into());
                    }
                }
            }
        }
    }

    /// After a flush: replay the samples since the last restart point (the last one is the sample
    /// being decoded) into the flushed codec.
    fn replay(&mut self) -> std::result::Result<(), String> {
        self.ended = false;
        let Some(log) = self.log.take() else {
            return Err("too far past a key frame to continue after a flush".into());
        };
        self.first = true;
        self.skip_rasl = false;
        let mut result = Ok(());
        for (s, p) in &log {
            if self.skips(s) {
                continue;
            }
            if let Err(e) = self.feed(s, *p) {
                result = Err(e);
                break;
            }
        }
        self.log = Some(log);
        result
    }

    fn take_out(&mut self) -> Vec<DecodedFrame> {
        let mut out = std::mem::take(&mut self.out);
        out.sort_by_key(|f| f.pts);
        out
    }
}

impl VideoDecoder for McDecoder {
    fn decode(&mut self, sample: &[u8], pts: i64) -> Result<Vec<DecodedFrame>> {
        self.remember(sample, pts);
        if self.ended {
            self.replay().map_err(CodecError::Decode)?;
            return Ok(self.take_out());
        }
        if self.skips(sample) {
            self.first = false;
            return Ok(Vec::new());
        }
        self.feed(sample, pts).map_err(CodecError::Decode)?;
        Ok(self.take_out())
    }

    fn flush(&mut self) -> Vec<DecodedFrame> {
        if !self.first && !self.ended {
            let drained = if self.session.is_some() { self.queue(None, 0).and_then(|()| self.drain(true)) } else { Ok(()) };
            if let Err(e) = drained {
                log::warn!("{}: {e} while flushing", self.name);
            }
            // the next sample replays the log, into a new codec if this one can't be flushed
            self.ended = true;
            if !matches!(self.session.as_ref().map(|s| s.codec.flush()), Some(Ok(()))) {
                self.session = None;
            }
            if let Some(s) = self.session.as_ref() {
                s.discard_images();
            }
        }
        self.take_out()
    }

    fn reset(&mut self) {
        match self.session.as_ref().map(|s| s.codec.flush()) {
            Some(Ok(())) => {}
            _ => self.session = None,
        }
        if let Some(s) = self.session.as_ref() {
            s.discard_images();
        }
        self.first = true;
        self.skip_rasl = false;
        self.ended = false;
        self.log = Some(Vec::new());
        self.log_bytes = 0;
        self.last_irap = 0;
        self.emitted.clear();
        self.out.clear();
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn is_random_access(&self, sample: &[u8]) -> Option<bool> {
        self.info.is_random_access(sample)
    }

    fn is_disposable(&self, sample: &[u8]) -> bool {
        self.info.is_disposable(sample)
    }
}

/// One plane of an image: its bytes and strides.
struct Plane<'a> {
    data: &'a [u8],
    row_stride: usize,
    pixel_stride: usize,
}

impl<'a> Plane<'a> {
    fn of(img: &'a Image, i: i32) -> std::result::Result<Plane<'a>, String> {
        let data = img.plane_data(i).map_err(|e| format!("plane {i}: {e:?}"))?;
        let row_stride = img.plane_row_stride(i).ok().and_then(|s| usize::try_from(s).ok()).ok_or("bad row stride")?;
        let pixel_stride = img.plane_pixel_stride(i).ok().and_then(|s| usize::try_from(s).ok()).filter(|&s| s >= 1).ok_or("bad pixel stride")?;
        Ok(Plane { data, row_stride, pixel_stride })
    }

    /// Append the `w`×`h` samples at `x`, `y` to `out`, row by row.
    fn copy(&self, x: usize, y: usize, w: usize, h: usize, out: &mut Vec<u8>) -> std::result::Result<(), String> {
        let ps = self.pixel_stride;
        for r in 0..h {
            let start = y
                .checked_add(r)
                .and_then(|row| row.checked_mul(self.row_stride))
                .and_then(|o| o.checked_add(x.checked_mul(ps)?))
                .ok_or("plane offset overflows")?;
            // the last sample of a row is the last byte read (an interleaved plane's buffer can end
            // right after it)
            let end = w.checked_sub(1).and_then(|n| n.checked_mul(ps)).and_then(|n| n.checked_add(start + 1)).ok_or("plane offset overflows")?;
            let row = self.data.get(start..end).ok_or("plane row out of range")?;
            if ps == 1 {
                out.extend_from_slice(row);
            } else {
                out.extend(row.iter().step_by(ps));
            }
        }
        Ok(())
    }
}

/// The picture in `img` as the software decoder would return it: the cropped rectangle of the
/// stream's sequence parameter set, as planar 8-bit 4:2:0.
fn to_frame(img: &Image, info: &NalStreamInfo) -> std::result::Result<VideoFrame, String> {
    let (cx, cy, w, h) = info.crop;
    // where the picture is in the image: the image's crop rectangle when it has the picture's size
    // (decoders that align the surface keep it there), else the stream's own crop offset
    let (ox, oy) = match img.crop_rect() {
        Ok(r)
            if r.left >= 0
                && r.top >= 0
                && i64::from(r.right) - i64::from(r.left) == i64::from(w)
                && i64::from(r.bottom) - i64::from(r.top) == i64::from(h) =>
        {
            (r.left as usize, r.top as usize)
        }
        _ => (cx as usize, cy as usize),
    };
    let (w, h) = (w as usize, h as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (luma, cb, cr) = (Plane::of(img, 0)?, Plane::of(img, 1)?, Plane::of(img, 2)?);
    let mut y = pool::take_u8(w * h);
    luma.copy(ox, oy, w, h, &mut y)?;
    let (mut u, mut v) = (pool::take_u8(cw * ch), pool::take_u8(cw * ch));
    cb.copy(ox / 2, oy / 2, cw, ch, &mut u)?;
    cr.copy(ox / 2, oy / 2, cw, ch, &mut v)?;
    let planes = [std::sync::Arc::new(y), std::sync::Arc::new(u), std::sync::Arc::new(v)];
    Ok(VideoFrame {
        width: w as u32,
        height: h as u32,
        data: PixelData::Yuv8 { planes, chroma: Chroma::C420, alpha: None },
        color: info.color,
        par: info.par,
        pts: filmcraft_time::Tick::ZERO,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_codec_names_are_recognised() {
        assert!(is_software_codec("c2.android.avc.decoder"));
        assert!(is_software_codec("OMX.google.h264.decoder"));
        assert!(!is_software_codec("c2.qti.avc.decoder"));
        assert!(!is_software_codec("OMX.Exynos.avc.dec"));
    }
}
