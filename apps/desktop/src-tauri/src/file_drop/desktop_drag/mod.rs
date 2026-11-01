use tokio::sync::mpsc::UnboundedSender;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "macos")]
mod session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragSignal {
    Started,
    Ended,
}

pub struct SignalEmitter {
    active: bool,
    sender: UnboundedSender<DragSignal>,
}

impl SignalEmitter {
    pub fn new(sender: UnboundedSender<DragSignal>) -> Self {
        Self {
            active: false,
            sender,
        }
    }

    pub fn update(&mut self, active: bool) -> bool {
        if active == self.active {
            return true;
        }
        self.active = active;
        let signal = if active {
            DragSignal::Started
        } else {
            DragSignal::Ended
        };
        self.sender.send(signal).is_ok()
    }
}

pub fn spawn(sender: UnboundedSender<DragSignal>) {
    #[cfg(any(windows, target_os = "macos"))]
    platform::spawn(sender);
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = sender;
}
