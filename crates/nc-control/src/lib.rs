pub mod config;
pub mod connection;
pub mod event;

mod driver;
mod wire;

pub use config::{ConnectionPurpose, ControlConfig, PairingIntent};
pub use connection::ControlConnection;
pub use event::ControlEvent;
