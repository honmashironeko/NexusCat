use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, TrySendError};
use nc_core::Result;
use tracing::{debug, warn};

use crate::CapturedFrame;

const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(1);
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(5);

pub(crate) fn run_frame_loop<F>(
    fps: u32,
    sink: &Sender<CapturedFrame>,
    running: &AtomicBool,
    mut capture: F,
) where
    F: FnMut() -> Result<CapturedFrame>,
{
    let period = Duration::from_nanos(1_000_000_000 / u64::from(fps.max(1)));
    let mut next_frame = Instant::now();
    let mut last_error_log: Option<Instant> = None;

    while running.load(Ordering::SeqCst) {
        match capture() {
            Ok(frame) => match sink.try_send(frame) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    debug!("屏幕帧接收端拥塞，丢弃一帧");
                }
                Err(TrySendError::Disconnected(_)) => {
                    debug!("屏幕帧接收端已关闭，停止采集");
                    break;
                }
            },
            Err(error) => {
                if last_error_log.is_none_or(|last| last.elapsed() >= ERROR_LOG_INTERVAL) {
                    warn!(%error, "屏幕采集失败，将继续重试");
                    last_error_log = Some(Instant::now());
                }
            }
        }

        next_frame += period;
        let now = Instant::now();
        if next_frame > now {
            sleep_until(next_frame, running);
        } else {
            next_frame = now;
        }
    }
}

fn sleep_until(deadline: Instant, running: &AtomicBool) {
    while running.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        std::thread::sleep((deadline - now).min(STOP_POLL_INTERVAL));
    }
}
