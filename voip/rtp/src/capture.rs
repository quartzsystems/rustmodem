// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Call captures: the `.rmcap` file, and what can be read out of one.
//!
//! A capture is every RTP packet a call's media thread received and sent,
//! in the order the thread handled them, each with the time on the
//! thread's monotonic clock. `docs/CAPTURE.md` is the reference for the
//! format; this module writes it, reads it, checks it ([`check`]) and
//! turns it into sound ([`write_wav`]).
//!
//! All numbers in the file are little-endian.

use std::fmt;
use std::io::{self, Read, Write};

use crate::{FRAME, RtpHeader, RtpReceiver, Verdict};

/// What a capture file starts with.
pub const MAGIC: [u8; 6] = *b"RMCAP\0";

/// The version of the format this module writes and reads.
pub const VERSION: u16 = 1;

/// The longest payload a record holds: 30 ms of G.711.
pub const MAX_PAYLOAD: usize = 3 * FRAME;

/// The longest call id a header holds.
pub const MAX_CALL_ID: usize = 255;

/// The header of a capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// The RTP payload type of the call: 0 for PCMU, 8 for PCMA.
    pub payload_type: u8,
    /// The packet time in milliseconds.
    pub ptime_ms: u8,
    /// The line the call was on.
    pub line: u16,
    /// When the capture began, in milliseconds since the Unix epoch.
    pub started_unix_ms: u64,
    /// The SIP Call-ID of the call.
    pub call_id: String,
}

/// What a record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A packet that arrived from the far end.
    Received,
    /// A packet that was sent to the far end.
    Sent,
    /// Records are missing here: the writer fell behind and the media
    /// thread dropped them. The record's timestamp field is how many.
    Gap,
}

impl Kind {
    fn code(self) -> u8 {
        match self {
            Kind::Received => 0,
            Kind::Sent => 1,
            Kind::Gap => 2,
        }
    }

    fn from_code(code: u8) -> Option<Kind> {
        match code {
            0 => Some(Kind::Received),
            1 => Some(Kind::Sent),
            2 => Some(Kind::Gap),
            _ => None,
        }
    }
}

/// One record of a capture: a packet, and when. It is a plain value of a
/// fixed size, so that a media thread can hand it over without allocating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: Kind,
    /// Nanoseconds on the media thread's monotonic clock since the call's
    /// media began.
    pub nanos: u64,
    pub header: RtpHeader,
    len: u16,
    payload: [u8; MAX_PAYLOAD],
}

impl Record {
    /// A record of a packet. A payload longer than [`MAX_PAYLOAD`] is cut
    /// to it.
    pub fn new(kind: Kind, nanos: u64, header: RtpHeader, payload: &[u8]) -> Record {
        let len = payload.len().min(MAX_PAYLOAD);
        let mut held = [0u8; MAX_PAYLOAD];
        held[..len].copy_from_slice(&payload[..len]);
        Record {
            kind,
            nanos,
            header,
            len: len as u16,
            payload: held,
        }
    }

    /// The record that says `dropped` records are missing here.
    pub fn gap(nanos: u64, dropped: u32) -> Record {
        let header = RtpHeader {
            marker: false,
            payload_type: 0,
            sequence: 0,
            timestamp: dropped,
            ssrc: 0,
        };
        Record::new(Kind::Gap, nanos, header, &[])
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.len)]
    }

    /// For a [`Kind::Gap`] record: how many records are missing.
    pub fn dropped(&self) -> u32 {
        self.header.timestamp
    }
}

/// Why a capture could not be read.
#[derive(Debug)]
pub enum CaptureError {
    /// The file could not be read.
    Io(io::Error),
    /// The file does not start with the magic of a capture.
    NotACapture,
    /// The capture is of a version this program does not read.
    Version(u16),
    /// The header or a record is not what the format says, at this offset.
    Malformed(u64),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::Io(error) => write!(f, "{error}"),
            CaptureError::NotACapture => write!(f, "the file is not a call capture"),
            CaptureError::Version(version) => {
                write!(f, "the capture is of version {version}, which is not known")
            }
            CaptureError::Malformed(offset) => {
                write!(f, "the capture is damaged at octet {offset}")
            }
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<io::Error> for CaptureError {
    fn from(error: io::Error) -> CaptureError {
        CaptureError::Io(error)
    }
}

/// Writes a capture.
pub struct Writer<W: Write> {
    out: W,
}

impl<W: Write> Writer<W> {
    /// Writes the header. A call id longer than [`MAX_CALL_ID`] octets is
    /// cut to it.
    pub fn new(mut out: W, header: &Header) -> io::Result<Writer<W>> {
        let mut call_id = header.call_id.as_bytes();
        if call_id.len() > MAX_CALL_ID {
            call_id = &call_id[..MAX_CALL_ID];
        }
        out.write_all(&MAGIC)?;
        out.write_all(&VERSION.to_le_bytes())?;
        out.write_all(&[header.payload_type, header.ptime_ms])?;
        out.write_all(&header.line.to_le_bytes())?;
        out.write_all(&header.started_unix_ms.to_le_bytes())?;
        out.write_all(&(call_id.len() as u16).to_le_bytes())?;
        out.write_all(call_id)?;
        Ok(Writer { out })
    }

    pub fn record(&mut self, record: &Record) -> io::Result<()> {
        let payload = record.payload();
        let mut fixed = [0u8; 24];
        fixed[0] = record.kind.code();
        fixed[1] = u8::from(record.header.marker);
        fixed[2] = record.header.payload_type;
        fixed[4..12].copy_from_slice(&record.nanos.to_le_bytes());
        fixed[12..14].copy_from_slice(&record.header.sequence.to_le_bytes());
        fixed[14..18].copy_from_slice(&record.header.timestamp.to_le_bytes());
        fixed[18..22].copy_from_slice(&record.header.ssrc.to_le_bytes());
        fixed[22..24].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        self.out.write_all(&fixed)?;
        self.out.write_all(payload)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    /// The writer's output, to finish with it.
    pub fn into_inner(self) -> W {
        self.out
    }
}

/// Reads a capture.
pub struct Reader<R: Read> {
    input: R,
    header: Header,
    offset: u64,
}

impl<R: Read> Reader<R> {
    /// Reads the header.
    pub fn new(mut input: R) -> Result<Reader<R>, CaptureError> {
        let mut fixed = [0u8; 22];
        read_exact(&mut input, &mut fixed, 0).map_err(|error| match error {
            // Too short to be anything.
            CaptureError::Malformed(_) => CaptureError::NotACapture,
            other => other,
        })?;
        if fixed[..6] != MAGIC {
            return Err(CaptureError::NotACapture);
        }
        let version = u16::from_le_bytes([fixed[6], fixed[7]]);
        if version != VERSION {
            return Err(CaptureError::Version(version));
        }
        let call_id_len = usize::from(u16::from_le_bytes([fixed[20], fixed[21]]));
        if call_id_len > MAX_CALL_ID {
            return Err(CaptureError::Malformed(20));
        }
        let mut call_id = vec![0u8; call_id_len];
        read_exact(&mut input, &mut call_id, 22)?;
        let header = Header {
            payload_type: fixed[8],
            ptime_ms: fixed[9],
            line: u16::from_le_bytes([fixed[10], fixed[11]]),
            started_unix_ms: u64::from_le_bytes([
                fixed[12], fixed[13], fixed[14], fixed[15], fixed[16], fixed[17], fixed[18],
                fixed[19],
            ]),
            call_id: String::from_utf8(call_id).map_err(|_| CaptureError::Malformed(22))?,
        };
        Ok(Reader {
            input,
            header,
            offset: 22 + call_id_len as u64,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The next record. `None` at the end of the file.
    pub fn record(&mut self) -> Result<Option<Record>, CaptureError> {
        let mut fixed = [0u8; 24];
        // The end of the file is the end of the capture only between
        // records.
        let got = self.input.read(&mut fixed[..1])?;
        if got == 0 {
            return Ok(None);
        }
        read_exact(&mut self.input, &mut fixed[1..], self.offset)?;
        let kind = Kind::from_code(fixed[0]).ok_or(CaptureError::Malformed(self.offset))?;
        let len = usize::from(u16::from_le_bytes([fixed[22], fixed[23]]));
        if len > MAX_PAYLOAD || fixed[1] > 1 {
            return Err(CaptureError::Malformed(self.offset));
        }
        let mut payload = [0u8; MAX_PAYLOAD];
        read_exact(&mut self.input, &mut payload[..len], self.offset + 24)?;
        self.offset += 24 + len as u64;
        let header = RtpHeader {
            marker: fixed[1] == 1,
            payload_type: fixed[2],
            sequence: u16::from_le_bytes([fixed[12], fixed[13]]),
            timestamp: u32::from_le_bytes([fixed[14], fixed[15], fixed[16], fixed[17]]),
            ssrc: u32::from_le_bytes([fixed[18], fixed[19], fixed[20], fixed[21]]),
        };
        let nanos = u64::from_le_bytes([
            fixed[4], fixed[5], fixed[6], fixed[7], fixed[8], fixed[9], fixed[10], fixed[11],
        ]);
        Ok(Some(Record::new(kind, nanos, header, &payload[..len])))
    }
}

/// Fills `buffer`, or says the capture is damaged at `offset`: a file that
/// ends inside a header or a record was cut off.
fn read_exact<R: Read>(input: &mut R, buffer: &mut [u8], offset: u64) -> Result<(), CaptureError> {
    match input.read_exact(buffer) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            Err(CaptureError::Malformed(offset))
        }
        Err(error) => Err(CaptureError::Io(error)),
    }
}

/// What [`check`] finds in a capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Packets from the far end that were delivered.
    pub received: u64,
    /// Packets sent to the far end.
    pub sent: u64,
    /// Packets from the far end that never arrived in time.
    pub lost: u64,
    pub late: u64,
    pub duplicate: u64,
    pub resync: u64,
    /// Packets of another payload type, or not whole frames.
    pub ignored: u64,
    /// The longest time between two arrivals from the far end, in
    /// microseconds.
    pub largest_gap_us: u64,
    /// Points where the samples sent stopped matching the samples
    /// received: a frame was sent that no received frame accounts for, or
    /// a received frame was never answered with one.
    pub slips: u64,
    /// Frames sent from the local timer before the far end's first packet.
    pub free_run: u64,
    /// Records the media thread dropped because the writer fell behind.
    /// What is counted around them cannot be trusted.
    pub dropped_records: u64,
}

impl Report {
    /// Whether the call was sample-exact as far as the capture shows:
    /// nothing lost, no resynchronisation, no slip, and no hole in the
    /// capture itself.
    pub fn is_clean(&self) -> bool {
        self.lost == 0 && self.resync == 0 && self.slips == 0 && self.dropped_records == 0
    }
}

/// Reads a capture through and counts what happened in it.
///
/// The received packets are replayed through an [`RtpReceiver`], the same
/// one the media thread used, so the counts are the ones the thread made.
/// The slips are counted from the two directions together: between two
/// arrivals, the frames sent must be the frames delivered, give or take
/// what was waiting to fill a packet.
pub fn check<R: Read>(reader: &mut Reader<R>) -> Result<Report, CaptureError> {
    let mut receiver = RtpReceiver::new(reader.header().payload_type);
    let mut report = Report::default();
    // Frames delivered and frames sent since the far end's first packet,
    // or since the last slip or hole.
    let mut delivered = 0u64;
    let mut sent = 0u64;
    // The frames in the last packet sent: the most that may be waiting.
    let mut per_packet = (u64::from(reader.header().ptime_ms) / 10).max(1);
    let mut synced = false;
    let mut last_arrival: Option<u64> = None;

    // Whether the frames sent account for the frames delivered: none
    // sent that was not delivered, and less than a packet's worth waiting.
    let balanced = |delivered: u64, sent: u64, per_packet: u64| {
        sent <= delivered && delivered - sent < per_packet
    };

    while let Some(record) = reader.record()? {
        match record.kind {
            Kind::Gap => {
                report.dropped_records += u64::from(record.dropped());
                // What was sent and received in the hole is unknown: the
                // count starts again.
                delivered = 0;
                sent = 0;
                receiver.forget_stream();
            }
            Kind::Received => {
                // Everything the last arrival caused has been sent by now.
                if synced && !balanced(delivered, sent, per_packet) {
                    report.slips += 1;
                    delivered = 0;
                    sent = 0;
                }
                if let Some(last) = last_arrival {
                    report.largest_gap_us = report
                        .largest_gap_us
                        .max(record.nanos.saturating_sub(last) / 1000);
                }
                last_arrival = Some(record.nanos);
                let arrival = (record.nanos / 125_000) as u32;
                let verdict = receiver.receive(&record.header, record.payload().len(), arrival);
                if let Verdict::Deliver {
                    fill_frames,
                    frames,
                    ..
                } = verdict
                {
                    if !synced {
                        synced = true;
                        sent = 0;
                    }
                    delivered += u64::from(fill_frames) + u64::from(frames);
                }
            }
            Kind::Sent => {
                let frames = (record.payload().len() / FRAME) as u64;
                report.sent += 1;
                if synced {
                    sent += frames;
                    per_packet = frames.max(1);
                } else {
                    report.free_run += frames;
                }
            }
        }
    }
    // A capture may end between an arrival and what was sent for it, so
    // frames still owed at the end are not a slip; frames sent that
    // nothing accounts for are.
    if synced && sent > delivered {
        report.slips += 1;
    }
    let stats = receiver.stats();
    report.received = stats.packets;
    report.lost = stats.lost;
    report.late = stats.late;
    report.duplicate = stats.duplicate;
    report.resync = stats.resync;
    report.ignored = stats.ignored;
    Ok(report)
}

/// Writes a capture as a two-channel G.711 WAV file: what was received on
/// the left and what was sent on the right. Gaps are filled, so that the
/// two channels stay aligned: a frame on the right is the frame sent for
/// the frame beside it on the left. Returns how many frames were written.
pub fn write_wav<R: Read, W: Write>(
    reader: &mut Reader<R>,
    out: &mut W,
) -> Result<u64, CaptureError> {
    let payload_type = reader.header().payload_type;
    let silence = if payload_type == 8 { 0xD5 } else { 0xFF };
    let mut receiver = RtpReceiver::new(payload_type);
    let mut left: Vec<u8> = Vec::new();
    let mut right: Vec<u8> = Vec::new();

    while let Some(record) = reader.record()? {
        match record.kind {
            Kind::Gap => receiver.forget_stream(),
            Kind::Received => {
                let arrival = (record.nanos / 125_000) as u32;
                if let Verdict::Deliver { fill_frames, .. } =
                    receiver.receive(&record.header, record.payload().len(), arrival)
                {
                    // Before an arrival the right channel has caught up
                    // with the left, but for what waits to fill a packet;
                    // a left channel that is behind had frames sent with
                    // nothing received: the local timer's.
                    if left.len() < right.len() {
                        left.resize(right.len(), silence);
                    }
                    left.resize(left.len() + fill_frames as usize * FRAME, silence);
                    left.extend_from_slice(record.payload());
                }
            }
            Kind::Sent => right.extend_from_slice(record.payload()),
        }
    }
    let samples = left.len().max(right.len());
    left.resize(samples, silence);
    right.resize(samples, silence);

    // RIFF/WAVE with the non-PCM format chunk: G.711 has format tag 7
    // (mu-law) or 6 (A-law), one octet a sample.
    let format: u16 = if payload_type == 8 { 6 } else { 7 };
    let data_len = (2 * samples) as u32;
    out.write_all(b"RIFF")?;
    out.write_all(&(4 + 26 + 12 + 8 + data_len).to_le_bytes())?;
    out.write_all(b"WAVE")?;
    out.write_all(b"fmt ")?;
    out.write_all(&18u32.to_le_bytes())?;
    out.write_all(&format.to_le_bytes())?;
    out.write_all(&2u16.to_le_bytes())?; // channels
    out.write_all(&8000u32.to_le_bytes())?; // samples a second
    out.write_all(&16_000u32.to_le_bytes())?; // octets a second
    out.write_all(&2u16.to_le_bytes())?; // block align
    out.write_all(&8u16.to_le_bytes())?; // bits a sample
    out.write_all(&0u16.to_le_bytes())?; // no extension
    out.write_all(b"fact")?;
    out.write_all(&4u32.to_le_bytes())?;
    out.write_all(&(samples as u32).to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    let mut interleaved = Vec::with_capacity(2 * samples);
    for (l, r) in left.iter().zip(&right) {
        interleaved.push(*l);
        interleaved.push(*r);
    }
    out.write_all(&interleaved)?;
    Ok((samples / FRAME) as u64)
}
