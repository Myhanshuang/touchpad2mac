//! Frame sinks for the CLI: a JSON-Lines frame printer (replay) and a
//! counting sink (record status).

use std::io::Write;

use touchpad_core::ContactFrame;
use touchpad_linux::FrameSink;

/// A [`FrameSink`] that prints each committed [`ContactFrame`] as one JSON
/// line on the output writer (the stable, testable replay output format:
/// stdout carries exactly one `serde_json` `ContactFrame` per line).
///
/// Frame serialization cannot fail for the core types; an output **write**
/// failure (e.g. stdout closed) is captured in [`FramePrinterSink::write_failed`]
/// and reported by the replay command, so a closed pipe never panics.
pub struct FramePrinterSink<'a> {
    out: &'a mut dyn Write,
    frames_written: u64,
    write_failed: bool,
}

impl<'a> FramePrinterSink<'a> {
    /// Creates a sink writing JSON frames to `out`.
    #[must_use]
    pub fn new(out: &'a mut dyn Write) -> Self {
        Self {
            out,
            frames_written: 0,
            write_failed: false,
        }
    }

    /// Number of frames successfully written.
    #[must_use]
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Whether a frame write failed (stdout closed / write error).
    #[must_use]
    pub fn write_failed(&self) -> bool {
        self.write_failed
    }
}

impl FrameSink for FramePrinterSink<'_> {
    fn on_frame(&mut self, frame: ContactFrame) {
        match serde_json::to_string(&frame) {
            Ok(json) => {
                if writeln!(self.out, "{json}").is_err() {
                    self.write_failed = true;
                } else {
                    self.frames_written += 1;
                }
            }
            Err(_) => {
                // The core types are always serializable; treat a failure as
                // an output failure rather than panicking.
                self.write_failed = true;
            }
        }
    }
}

/// A [`FrameSink`] that only counts what the decoder published (used by
/// `record` for the exit status: the trace carries the raw events, the
/// frames are incidental pipeline validation).
#[derive(Clone, Debug, Default)]
pub struct CountingSink {
    frames: u64,
    contacts: u64,
    discontinuities: u64,
    diagnostics: u64,
}

impl CountingSink {
    /// Creates an empty counting sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of frames published.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Number of contacts published (across all frames).
    #[must_use]
    pub fn contacts(&self) -> u64 {
        self.contacts
    }

    /// Number of discontinuity frames published.
    #[must_use]
    pub fn discontinuities(&self) -> u64 {
        self.discontinuities
    }

    /// Number of diagnostics attached to published frames.
    #[must_use]
    pub fn diagnostics(&self) -> u64 {
        self.diagnostics
    }
}

impl FrameSink for CountingSink {
    fn on_frame(&mut self, frame: ContactFrame) {
        self.frames += 1;
        self.contacts += frame.contacts.len() as u64;
        if frame.discontinuity {
            self.discontinuities += 1;
        }
        self.diagnostics += frame.diagnostics.len() as u64;
    }
}
