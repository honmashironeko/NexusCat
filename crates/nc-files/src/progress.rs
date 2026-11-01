use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FileProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub percent: f64,
}

impl FileProgress {
    pub(crate) const fn new(bytes_done: u64, bytes_total: u64) -> Self {
        let percent = if bytes_total == 0 {
            100.0
        } else {
            bytes_done as f64 * 100.0 / bytes_total as f64
        };
        Self {
            bytes_done,
            bytes_total,
            percent,
        }
    }
}
