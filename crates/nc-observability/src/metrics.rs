use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::rolling::{RollingHistogram, SlidingRateWindow};

const RATE_WINDOW: Duration = Duration::from_secs(1);
const LATENCY_WINDOW: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LatencySample {
    pub capture_us: u64,
    pub encode_us: u64,
    pub network_us: u64,
    pub decode_us: u64,
    pub present_us: u64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub fps: f64,
    pub capture_fps: f64,
    pub bitrate_kbps: f64,
    pub loss_percent: f64,
    pub rtt_ms: f64,
    pub received_fps: f64,
    pub presented_fps: f64,
    pub capture_ms: f64,
    pub encode_ms: f64,
    pub network_ms: f64,
    pub decode_ms: f64,
    pub present_ms: f64,
    pub first_frame_ms: Option<f64>,
    pub glass_to_glass_p50_ms: Option<f64>,
    pub glass_to_glass_p95_ms: Option<f64>,
    pub nack_count: u64,
    pub frames_sent: u64,
    pub frames_received: u64,
    pub frames_presented: u64,
    pub dropped_frames: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug)]
pub struct MetricsRegistry {
    inner: Mutex<RegistryInner>,
}

#[derive(Debug)]
struct RegistryInner {
    start: Instant,
    rate: SlidingRateWindow,
    capture_rate: SlidingRateWindow,
    received_rate: SlidingRateWindow,
    presented_rate: SlidingRateWindow,
    capture: RollingHistogram,
    encode: RollingHistogram,
    network: RollingHistogram,
    decode: RollingHistogram,
    present: RollingHistogram,
    glass_to_glass: RollingHistogram,
    rtt_ms: f64,
    loss_percent: f64,
    first_frame_ms: Option<f64>,
    nack_count: u64,
    frames_sent: u64,
    frames_received: u64,
    frames_presented: u64,
    dropped_frames: u64,
}

impl RegistryInner {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            rate: SlidingRateWindow::new(RATE_WINDOW),
            capture_rate: SlidingRateWindow::new(RATE_WINDOW),
            received_rate: SlidingRateWindow::new(RATE_WINDOW),
            presented_rate: SlidingRateWindow::new(RATE_WINDOW),
            capture: RollingHistogram::new(LATENCY_WINDOW),
            encode: RollingHistogram::new(LATENCY_WINDOW),
            network: RollingHistogram::new(LATENCY_WINDOW),
            decode: RollingHistogram::new(LATENCY_WINDOW),
            present: RollingHistogram::new(LATENCY_WINDOW),
            glass_to_glass: RollingHistogram::new(LATENCY_WINDOW),
            rtt_ms: 0.0,
            loss_percent: 0.0,
            first_frame_ms: None,
            nack_count: 0,
            frames_sent: 0,
            frames_received: 0,
            frames_presented: 0,
            dropped_frames: 0,
        }
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RegistryInner::new()),
        }
    }

    pub fn record_capture_frame(&self) {
        let mut inner = self.inner.lock();
        let now = inner.start.elapsed();
        inner.capture_rate.record(now, 1);
    }

    pub fn record_video_frame(&self, sent_bytes: u64, encode_us: u64, capture_us: u64) {
        let mut inner = self.inner.lock();
        let now = inner.start.elapsed();
        inner.rate.record(now, sent_bytes);
        inner.frames_sent += 1;
        inner.capture.push(us_to_ms(capture_us));
        inner.encode.push(us_to_ms(encode_us));
    }

    pub fn record_received_frames(
        &self,
        frames: u32,
        decode_us: u64,
        present_us: u64,
        dropped_frames: u32,
    ) {
        let mut inner = self.inner.lock();
        let frames = u64::from(frames.min(1_000));
        inner.frames_received = inner.frames_received.saturating_add(frames);
        inner.dropped_frames = inner
            .dropped_frames
            .saturating_add(u64::from(dropped_frames.min(1_000)));
        let now = inner.start.elapsed();
        for _ in 0..frames {
            inner.received_rate.record(now, 1);
        }
        inner.decode.push(us_to_ms(decode_us));
        inner.present.push(us_to_ms(present_us));
    }

    pub fn record_presented_frames(&self, frames: u32) {
        let mut inner = self.inner.lock();
        let frames = u64::from(frames.min(1_000));
        inner.frames_presented = inner.frames_presented.saturating_add(frames);
        let now = inner.start.elapsed();
        for _ in 0..frames {
            inner.presented_rate.record(now, 1);
        }
    }

    pub fn record_received_frame(&self, decode_us: u64, present_us: u64, dropped: bool) {
        self.record_received_frames(1, decode_us, present_us, u32::from(dropped));
    }

    pub fn record_latency(&self, sample: LatencySample) {
        let total_us = sample
            .capture_us
            .saturating_add(sample.encode_us)
            .saturating_add(sample.network_us)
            .saturating_add(sample.decode_us)
            .saturating_add(sample.present_us);

        let mut inner = self.inner.lock();
        inner.capture.push(us_to_ms(sample.capture_us));
        inner.encode.push(us_to_ms(sample.encode_us));
        inner.network.push(us_to_ms(sample.network_us));
        inner.decode.push(us_to_ms(sample.decode_us));
        inner.present.push(us_to_ms(sample.present_us));
        inner.glass_to_glass.push(us_to_ms(total_us));
    }

    pub fn record_network(&self, rtt_ms: f64, loss_percent: f64) {
        let mut inner = self.inner.lock();
        if rtt_ms > 0.0 {
            inner.rtt_ms = sanitize_non_negative(rtt_ms);
        }
        inner.loss_percent = sanitize_non_negative(loss_percent).min(100.0);
    }

    pub fn record_loss(&self, loss_percent: f64) {
        let mut inner = self.inner.lock();
        inner.loss_percent = sanitize_non_negative(loss_percent).min(100.0);
    }

    pub fn record_first_frame(&self, ms: f64) {
        self.inner.lock().first_frame_ms = Some(sanitize_non_negative(ms));
    }

    pub fn record_nack(&self) {
        self.inner.lock().nack_count += 1;
    }

    pub fn record_glass_to_glass(&self, ms: f64) {
        self.inner
            .lock()
            .glass_to_glass
            .push(sanitize_non_negative(ms));
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut inner = self.inner.lock();
        let now = inner.start.elapsed();
        let (fps, bitrate_kbps) = inner.rate.rates(now);
        let (capture_fps, _) = inner.capture_rate.rates(now);
        let (received_fps, _) = inner.received_rate.rates(now);
        let (presented_fps, _) = inner.presented_rate.rates(now);

        MetricsSnapshot {
            fps,
            capture_fps,
            received_fps,
            presented_fps,
            bitrate_kbps,
            loss_percent: inner.loss_percent,
            rtt_ms: inner.rtt_ms,
            capture_ms: inner.capture.mean().unwrap_or(0.0),
            encode_ms: inner.encode.mean().unwrap_or(0.0),
            network_ms: inner.network.mean().unwrap_or(0.0),
            decode_ms: inner.decode.mean().unwrap_or(0.0),
            present_ms: inner.present.mean().unwrap_or(0.0),
            first_frame_ms: inner.first_frame_ms,
            glass_to_glass_p50_ms: inner.glass_to_glass.percentile(0.5),
            glass_to_glass_p95_ms: inner.glass_to_glass.percentile(0.95),
            nack_count: inner.nack_count,
            frames_sent: inner.frames_sent,
            frames_received: inner.frames_received,
            frames_presented: inner.frames_presented,
            dropped_frames: inner.dropped_frames,
            updated_at_ms: now_ms(),
        }
    }

    pub fn reset(&self) {
        *self.inner.lock() = RegistryInner::new();
    }
}

fn us_to_ms(microseconds: u64) -> f64 {
    microseconds as f64 / 1000.0
}

fn sanitize_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
