//! Frame output contract for the decoder.
//!
//! The decoder publishes [`ContactFrame`]s through a [`FrameSink`]. Live
//! input (M4) will forward frames into the contact model / interaction
//! pipeline; tests and the offline replay path observe them with
//! [`RecordingFrameSink`].
#![forbid(unsafe_code)]

use touchpad_core::ContactFrame;

/// Consumer of committed [`ContactFrame`]s produced by the Type-B decoder.
///
/// Frames are published in order, exactly once per committed `SYN_REPORT`
/// (including resynchronization frames). A frame is only ever produced by the
/// decoder's commit logic — no other path may call the sink.
pub trait FrameSink {
    /// Publishes one committed frame.
    fn on_frame(&mut self, frame: ContactFrame);
}

/// A test/observation sink that records every published frame in order.
#[derive(Clone, Debug, Default)]
pub struct RecordingFrameSink {
    frames: Vec<ContactFrame>,
}

impl RecordingFrameSink {
    /// Creates an empty recording sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The frames recorded so far, in publication order.
    #[must_use]
    pub fn frames(&self) -> &[ContactFrame] {
        &self.frames
    }

    /// Takes the recorded frames, leaving the sink empty.
    #[must_use]
    pub fn take_frames(&mut self) -> Vec<ContactFrame> {
        std::mem::take(&mut self.frames)
    }

    /// Number of recorded frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether no frames were recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

impl FrameSink for RecordingFrameSink {
    fn on_frame(&mut self, frame: ContactFrame) {
        self.frames.push(frame);
    }
}
