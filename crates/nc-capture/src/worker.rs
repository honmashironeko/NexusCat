use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, RecvTimeoutError, Sender};
use nc_core::{NexusError, Result};
use tracing::warn;

pub(crate) type StartupReporter = Sender<std::result::Result<(), String>>;

pub(crate) struct CaptureWorker {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CaptureWorker {
    pub(crate) fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.thread.is_some() && self.running.load(Ordering::SeqCst)
    }

    pub(crate) fn spawn<F>(&mut self, name: &str, startup_timeout: Duration, work: F) -> Result<()>
    where
        F: FnOnce(Arc<AtomicBool>, StartupReporter) + Send + 'static,
    {
        if self.thread.is_some() {
            if self.running.load(Ordering::SeqCst) {
                return Err(NexusError::invalid("采集已在运行，不能重复启动"));
            }
            self.join_finished();
        }

        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let (startup_tx, startup_rx) = bounded(1);
        let spawned = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _running_guard = RunningGuard(worker_running.clone());
                work(worker_running, startup_tx);
            });

        match spawned {
            Ok(handle) => {
                self.thread = Some(handle);
                self.running = running;
            }
            Err(error) => {
                return Err(NexusError::platform(format!("创建采集线程失败：{error}")));
            }
        }

        match startup_rx.recv_timeout(startup_timeout) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => {
                self.stop();
                Err(NexusError::platform(message))
            }
            Err(RecvTimeoutError::Timeout) => {
                self.stop();
                Err(NexusError::platform("采集线程初始化超时"))
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.stop();
                Err(NexusError::platform("采集线程在初始化完成前退出"))
            }
        }
    }

    pub(crate) fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.join_finished();
    }

    fn join_finished(&mut self) {
        if let Some(handle) = self.thread.take() {
            if handle.join().is_err() {
                warn!("采集线程因 panic 退出");
            }
        }
    }
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

struct RunningGuard(Arc<AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
