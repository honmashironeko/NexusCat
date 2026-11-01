use std::collections::VecDeque;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RollingHistogram {
    capacity: usize,
    values: VecDeque<f64>,
}

impl RollingHistogram {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            values: VecDeque::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn push(&mut self, value: f64) {
        if self.capacity == 0 || !value.is_finite() {
            return;
        }
        if self.values.len() == self.capacity {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn mean(&self) -> Option<f64> {
        if self.values.is_empty() {
            return None;
        }
        let total: f64 = self.values.iter().sum();
        Some(total / self.values.len() as f64)
    }

    pub fn percentile(&self, percentile: f64) -> Option<f64> {
        if self.values.is_empty() || !(0.0..=1.0).contains(&percentile) {
            return None;
        }
        let mut sorted: Vec<f64> = self.values.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);

        if sorted.len() == 1 {
            return Some(sorted[0]);
        }

        let rank = percentile * (sorted.len() - 1) as f64;
        let lower_index = rank.floor() as usize;
        let upper_index = rank.ceil() as usize;
        let fraction = rank - lower_index as f64;
        let lower = sorted[lower_index];
        Some(lower + (sorted[upper_index] - lower) * fraction)
    }
}

#[derive(Debug)]
pub(crate) struct SlidingRateWindow {
    window: Duration,
    samples: VecDeque<(Duration, u64)>,
}

impl SlidingRateWindow {
    pub(crate) fn new(window: Duration) -> Self {
        Self {
            window,
            samples: VecDeque::new(),
        }
    }

    pub(crate) fn record(&mut self, now: Duration, bytes: u64) {
        self.prune(now);
        self.samples.push_back((now, bytes));
    }

    pub(crate) fn rates(&mut self, now: Duration) -> (f64, f64) {
        self.prune(now);
        let frames = self.samples.len() as f64;
        let bytes: u64 = self.samples.iter().map(|(_, bytes)| *bytes).sum();
        (frames, bytes as f64 * 8.0 / 1000.0)
    }

    fn prune(&mut self, now: Duration) {
        while let Some((timestamp, _)) = self.samples.front() {
            if now.saturating_sub(*timestamp) > self.window {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }
}
