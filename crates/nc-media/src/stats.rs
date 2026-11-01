use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStats {
    pub frames_sent: u64,
    pub frames_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub nack_sent: u64,
    pub nack_received: u64,
    pub dropped_frames: u64,
    pub duplicate_packets: u64,
    pub loss_percent: f32,
    pub last_rtt_ms: u64,
}

impl MediaStats {
    pub fn merge(&mut self, other: &Self) {
        self.frames_sent += other.frames_sent;
        self.frames_received += other.frames_received;
        self.bytes_sent += other.bytes_sent;
        self.bytes_received += other.bytes_received;
        self.nack_sent += other.nack_sent;
        self.nack_received += other.nack_received;
        self.dropped_frames += other.dropped_frames;
        self.duplicate_packets += other.duplicate_packets;
        if other.loss_percent.is_finite() {
            self.loss_percent = other.loss_percent;
        }
        if other.last_rtt_ms > 0 {
            self.last_rtt_ms = other.last_rtt_ms;
        }
    }

    pub(crate) fn set_loss(&mut self, lost_fragments: u64, expected_fragments: u64) {
        self.loss_percent = if expected_fragments == 0 {
            0.0
        } else {
            (lost_fragments as f64 * 100.0 / expected_fragments as f64) as f32
        };
    }
}
