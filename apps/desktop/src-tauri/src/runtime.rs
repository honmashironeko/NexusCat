use crate::actions::UiAction;
use crate::config::AppPaths;
use crate::ipc::{
    EdgeHaloView, FileDropCardsView, MediaBridgeInfo, TransferView, UpdateStatusView,
    WebCodecsProbe,
};
use crate::media_bridge::MediaBridge;
use crate::notifications::NotificationCenter;
use crate::tray::TrayHandle;
use nc_capture::{AudioCapturer, ScreenCapturer};
use nc_control::ControlConnection;
#[cfg(target_os = "macos")]
use nc_core::PermissionKind;
use nc_core::{
    AppSettings, DeviceId, DeviceInfo, Direction, DisplayNodeId, Point, SessionSnapshot, StreamId,
    VideoTarget,
};
use nc_crypto::{DeviceCertificate, Identity, TrustStore};
use nc_discovery::DiscoveryService;
use nc_files::{FileReceiver, FileSender};
use nc_input::{InputCapture, InputInjector};
use nc_media::{MediaConfig, MediaEvent, MediaSender};
use nc_observability::MetricsRegistry;
use nc_proto::{FileTransferId, StreamRequest};
use nc_session::SessionMachine;
use nc_topology::{EdgeConfig, EdgeDetector, Layout};
use nc_update::{UpdateManager, UpdateManifest, UpdateReceiver};
use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

const RELATIVE_MOVE_COALESCE: Duration = Duration::from_millis(4);
const ABSOLUTE_MOVE_COALESCE: Duration = Duration::from_millis(4);
const STAGE_EDGE_TOLERANCE_PX: f64 = 2.0;
const STAGE_EDGE_GRACE_MS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Host,
    Target,
}

pub struct PendingTakeover {
    pub host: DeviceId,
    pub target: DeviceId,
    pub started: Instant,
}

pub struct ActiveControl {
    pub id: u64,
    pub connection: Arc<ControlConnection>,
    pub peer: RwLock<Option<DeviceInfo>>,
    pub media_key: RwLock<Option<[u8; 32]>>,
    pub connected: AtomicBool,
    pub direction: ControlDirection,
}

pub struct AuxControl {
    pub connection: ControlConnection,
    pub peer: DeviceInfo,
}

impl ActiveControl {
    pub fn new(id: u64, connection: ControlConnection, direction: ControlDirection) -> Self {
        Self {
            id,
            connection: Arc::new(connection),
            peer: RwLock::new(None),
            media_key: RwLock::new(None),
            connected: AtomicBool::new(false),
            direction,
        }
    }

    pub fn peer_info(&self) -> Option<DeviceInfo> {
        self.peer.read().clone()
    }

    pub fn media_key(&self) -> Option<[u8; 32]> {
        *self.media_key.read()
    }
}

#[derive(Debug, Clone)]
pub struct PendingRemoteEntry {
    pub device: DeviceId,
    pub node: DisplayNodeId,
    pub entry: Point,
    pub scale: (f64, f64),
    pub cursor_offset: (f64, f64),
    pub direction: Direction,
}

#[derive(Debug, Clone, Default)]
pub struct PairingState {
    pub device_id: Option<DeviceId>,
    pub device_name: Option<String>,
    pub sas: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamBinding {
    pub device_id: DeviceId,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio: Option<AudioBinding>,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioBinding {
    pub codec: nc_core::AudioCodec,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct OutgoingStream {
    pub stream_id: StreamId,
    pub device_id: DeviceId,
    pub monitor_id: u32,
    pub stop: Arc<AtomicBool>,
    pub keyframe_requested: Arc<AtomicBool>,
    pub capture: Option<Box<dyn ScreenCapturer>>,
    pub audio: Option<OutgoingAudio>,
    pub encoder_task: Option<tokio::task::JoinHandle<()>>,
    pub send_task: Option<tokio::task::JoinHandle<()>>,
    pub capture_forwarder: Option<std::thread::JoinHandle<()>>,
}

pub struct OutgoingAudio {
    pub capturer: Box<dyn AudioCapturer>,
    pub stop: Arc<AtomicBool>,
    pub send_task: Option<tokio::task::JoinHandle<()>>,
    pub forwarder: Option<std::thread::JoinHandle<()>>,
}

impl OutgoingStream {
    pub fn stop_blocking(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(mut capture) = self.capture.take() {
            capture.stop();
        }
        if let Some(mut audio) = self.audio.take() {
            audio.stop.store(true, Ordering::SeqCst);
            audio.capturer.stop();
            if let Some(handle) = audio.forwarder.take() {
                let _ = handle.join();
            }
            if let Some(handle) = audio.send_task.take() {
                handle.abort();
            }
        }
        if let Some(handle) = self.capture_forwarder.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.encoder_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.send_task.take() {
            handle.abort();
        }
    }
}

pub struct MediaRuntime {
    pub recv_socket: Arc<UdpSocket>,
    pub media_port: u16,
    pub config: MediaConfig,
    pub sender: RwLock<Option<Arc<MediaSender>>>,
    pub receiver_task: Mutex<Option<JoinHandle<()>>>,
    pub bindings: RwLock<HashMap<StreamId, StreamBinding>>,
    pub outgoing: Mutex<HashMap<StreamId, OutgoingStream>>,
    pub peer_addr: RwLock<Option<SocketAddr>>,
    pub fec_parity: std::sync::atomic::AtomicU8,
    pub current_session_id: AtomicU64,
    pub stream_epoch: AtomicU64,
}

impl MediaRuntime {
    pub fn new(recv_socket: Arc<UdpSocket>, config: MediaConfig) -> std::io::Result<Self> {
        let media_port = recv_socket.local_addr()?.port();
        Ok(Self {
            recv_socket,
            media_port,
            config,
            sender: RwLock::new(None),
            receiver_task: Mutex::new(None),
            bindings: RwLock::new(HashMap::new()),
            outgoing: Mutex::new(HashMap::new()),
            peer_addr: RwLock::new(None),
            fec_parity: std::sync::atomic::AtomicU8::new(0),
            current_session_id: AtomicU64::new(0),
            stream_epoch: AtomicU64::new(0),
        })
    }

    pub fn sender(&self) -> Option<Arc<MediaSender>> {
        self.sender.read().clone()
    }

    pub fn set_fec_parity(&self, parity: u8) {
        self.fec_parity
            .store(parity, std::sync::atomic::Ordering::SeqCst);
        if let Some(sender) = self.sender() {
            sender.set_fec_parity(parity);
        }
    }

    pub fn fec_parity(&self) -> u8 {
        self.fec_parity.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_session_id(&self, session_id: u64) {
        self.current_session_id
            .store(session_id, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn session_id(&self) -> u64 {
        self.current_session_id
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn binding(&self, stream_id: StreamId) -> Option<StreamBinding> {
        self.bindings.read().get(&stream_id).cloned()
    }

    pub fn clear_session(&self) {
        *self.sender.write() = None;
        *self.peer_addr.write() = None;
        self.bindings.write().clear();
        self.set_session_id(0);
        if let Some(handle) = self.receiver_task.lock().take() {
            handle.abort();
        }
    }

    pub async fn stop_all_outgoing(&self) {
        let streams: Vec<_> = {
            let mut outgoing = self.outgoing.lock();
            outgoing.drain().map(|(_, stream)| stream).collect()
        };
        for mut stream in streams {
            let _ = tokio::task::spawn_blocking(move || stream.stop_blocking()).await;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StagePointerGeometry {
    pub stage_bounds: (f64, f64, f64, f64),
    pub stream_aspect: f64,
    pub remote_bounds: (f64, f64),
    pub cursor_offset: (f64, f64),
}

pub struct InputRuntime {
    pub capture: Option<Box<dyn InputCapture>>,
    pub injector: Option<Box<dyn InputInjector>>,
    pub suppressed: Arc<AtomicBool>,
    pub relative_mode: Arc<AtomicBool>,
    pub active: Arc<AtomicBool>,
    pub edge_detector: EdgeDetector,
    pub forwarder: Option<std::thread::JoinHandle<()>>,
    pub last_remote_event_ms: u64,
    pub last_injected: Option<(nc_core::InputEvent, Instant)>,
    pub virtual_cursor: Option<(f64, f64)>,
    pub virtual_bounds: Option<(f64, f64)>,
    pub remote_cursor_offset: (f64, f64),
    pub remote_device: Option<DeviceId>,
    pub remote_node: Option<DisplayNodeId>,
    pub local_entry_node: Option<DisplayNodeId>,
    pub last_cursor_emit_ms: u64,
    pub remote_scale: (f64, f64),
    pub remote_return_direction: Option<Direction>,
    pub remote_entry_direction: Option<Direction>,
    pub remote_inside: bool,
    pub remote_outward_events: u32,
    pub remote_outward_direction: Option<Direction>,
    pub control_started_ms: u64,
    pub last_escape_ms: u64,
    pub escape_down: bool,
    pub controlling_remote: bool,
    pub remote_input_released: bool,
    pub remote_handover_notified: bool,
    pub absolute_pointer: bool,
    pub stage_bounds: Option<(f64, f64, f64, f64)>,
    pub stream_aspect: Option<f64>,
    pub remote_paused: bool,
    pub absolute_edge_armed: bool,
    pub absolute_edge_generation: u64,
    pub shortcut_key_down: Option<u32>,
    pub pending_shortcut_action: Option<UiAction>,
}

impl Default for InputRuntime {
    fn default() -> Self {
        Self {
            capture: None,
            injector: None,
            suppressed: Arc::new(AtomicBool::new(false)),
            relative_mode: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(false)),
            edge_detector: EdgeDetector::new(EdgeConfig::default()),
            forwarder: None,
            last_remote_event_ms: 0,
            last_injected: None,
            virtual_cursor: None,
            virtual_bounds: None,
            remote_cursor_offset: (0.0, 0.0),
            remote_device: None,
            remote_node: None,
            local_entry_node: None,
            last_cursor_emit_ms: 0,
            remote_scale: (1.0, 1.0),
            remote_return_direction: None,
            remote_entry_direction: None,
            remote_inside: false,
            remote_outward_events: 0,
            remote_outward_direction: None,
            control_started_ms: 0,
            last_escape_ms: 0,
            escape_down: false,
            controlling_remote: false,
            remote_input_released: false,
            remote_handover_notified: false,
            absolute_pointer: false,
            stage_bounds: None,
            stream_aspect: None,
            remote_paused: false,
            absolute_edge_armed: false,
            absolute_edge_generation: 0,
            shortcut_key_down: None,
            pending_shortcut_action: None,
        }
    }
}

impl InputRuntime {
    pub fn note_remote_handover(&mut self) -> bool {
        if self.remote_handover_notified {
            false
        } else {
            self.remote_handover_notified = true;
            true
        }
    }

    fn install_capture_part(&mut self, part: InputCapturePart, events: UnboundedSender<AppEvent>) {
        if let Some(mut previous) = self.capture.take() {
            previous.stop();
        }
        if let Some(previous) = self.forwarder.take() {
            let _ = previous.join();
        }

        let InputCapturePart { capture, receiver } = part;
        capture.set_suppressed(self.suppressed.load(Ordering::SeqCst));
        capture.set_relative_mode(self.relative_mode.load(Ordering::SeqCst));
        capture.set_absolute_pointer(self.absolute_pointer);
        let active = Arc::clone(&self.active);
        self.capture = Some(capture);
        self.forwarder = Some(spawn_input_forwarder(receiver, active, events));
    }

    pub fn stop(&mut self) {
        if let Some(capture) = &mut self.capture {
            capture.stop();
        }
        if let Some(handle) = self.forwarder.take() {
            let _ = handle.join();
        }
        let _ = nc_platform::set_cursor_clip(None);
    }

    pub fn begin_remote_cursor(
        &mut self,
        entry: (f64, f64),
        bounds: (f64, f64),
        scale: (f64, f64),
        entry_direction: Direction,
        cursor_offset: (f64, f64),
    ) {
        self.virtual_cursor = Some(entry);
        self.virtual_bounds = Some(bounds);
        self.remote_cursor_offset = cursor_offset;
        self.remote_scale = scale;
        self.remote_return_direction = Some(entry_direction.opposite());
        self.remote_entry_direction = Some(entry_direction);
        self.remote_inside = false;
        self.remote_outward_events = 0;
        self.remote_outward_direction = None;
        self.control_started_ms = crate::notifications::now_ms();
        self.last_escape_ms = 0;
        self.escape_down = false;
        self.last_cursor_emit_ms = 0;
        self.controlling_remote = true;
        self.remote_input_released = false;
        self.remote_paused = false;
        self.set_absolute_pointer(false);
        self.reset_absolute_edge();
    }

    pub fn begin_absolute_stage_cursor(
        &mut self,
        geometry: StagePointerGeometry,
        cursor: (f64, f64),
        device: DeviceId,
        node: DisplayNodeId,
    ) {
        self.virtual_cursor = Some(cursor);
        self.virtual_bounds = Some(geometry.remote_bounds);
        self.remote_cursor_offset = geometry.cursor_offset;
        self.remote_device = Some(device);
        self.remote_node = Some(node);
        self.remote_scale = (1.0, 1.0);
        self.stage_bounds = Some(geometry.stage_bounds);
        self.stream_aspect = Some(geometry.stream_aspect);
        self.remote_inside = false;
        self.remote_outward_events = 0;
        self.remote_outward_direction = None;
        self.control_started_ms = crate::notifications::now_ms();
        self.last_escape_ms = 0;
        self.escape_down = false;
        self.last_cursor_emit_ms = 0;
        self.controlling_remote = true;
        self.remote_input_released = false;
        self.remote_paused = false;
        self.reset_absolute_edge();
        self.set_absolute_pointer(true);
        if let Some(capture) = &self.capture {
            capture.set_relative_mode(false);
            capture.set_suppressed(true);
        }
    }

    pub fn map_absolute_stage_pointer(&mut self, x: f64, y: f64) -> Option<(f64, f64)> {
        let stage_bounds = self.stage_bounds?;
        let stream_aspect = self.stream_aspect?;
        let remote_bounds = self.virtual_bounds?;
        let mapped = stage_pointer_to_remote(stage_bounds, stream_aspect, remote_bounds, x, y)?;
        self.virtual_cursor = Some(mapped);
        Some(mapped)
    }

    pub fn update_absolute_edge(
        &mut self,
        x: f64,
        y: f64,
        now_ms: u64,
    ) -> Option<(Direction, u64)> {
        let Some((stage_x, stage_y, stage_width, stage_height)) = self.stage_bounds else {
            self.cancel_absolute_edge();
            return None;
        };
        let direction = if x <= stage_x + STAGE_EDGE_TOLERANCE_PX {
            Some(Direction::Left)
        } else if x >= stage_x + stage_width - STAGE_EDGE_TOLERANCE_PX {
            Some(Direction::Right)
        } else if y <= stage_y + STAGE_EDGE_TOLERANCE_PX {
            Some(Direction::Up)
        } else if y >= stage_y + stage_height - STAGE_EDGE_TOLERANCE_PX {
            Some(Direction::Down)
        } else {
            None
        };
        let Some(direction) = direction else {
            self.remote_inside = true;
            self.cancel_absolute_edge();
            return None;
        };
        let can_leave = self.remote_inside
            || now_ms.saturating_sub(self.control_started_ms) >= STAGE_EDGE_GRACE_MS;
        if !can_leave || self.absolute_edge_armed {
            return None;
        }
        self.absolute_edge_armed = true;
        self.absolute_edge_generation = self.absolute_edge_generation.wrapping_add(1);
        Some((direction, self.absolute_edge_generation))
    }

    fn cancel_absolute_edge(&mut self) {
        if self.absolute_edge_armed {
            self.absolute_edge_armed = false;
            self.absolute_edge_generation = self.absolute_edge_generation.wrapping_add(1);
        }
    }

    fn reset_absolute_edge(&mut self) {
        self.absolute_edge_armed = false;
        self.absolute_edge_generation = self.absolute_edge_generation.wrapping_add(1);
    }

    pub fn latch_mouse_moves(&self, direction: Direction, duration_ms: u64) {
        if let Some(capture) = &self.capture {
            capture.latch_mouse_moves(direction, duration_ms);
        }
    }

    pub fn set_absolute_pointer(&mut self, enabled: bool) {
        self.absolute_pointer = enabled;
        if enabled {
            if let Some((x, y, width, height)) = self.stage_bounds {
                let bounds = nc_platform::RectI::new(
                    x.round() as i32,
                    y.round() as i32,
                    (width.round().max(1.0)) as u32,
                    (height.round().max(1.0)) as u32,
                );
                if let Err(error) = nc_platform::set_cursor_clip(Some(bounds)) {
                    tracing::debug!(%error, "限制光标在舞台显示器内失败");
                }
            }
        } else {
            self.stage_bounds = None;
            self.stream_aspect = None;
            self.reset_absolute_edge();
            if let Err(error) = nc_platform::set_cursor_clip(None) {
                tracing::debug!(%error, "取消光标限制失败");
            }
        }
        if let Some(capture) = &self.capture {
            capture.set_absolute_pointer(enabled);
        }
    }

    pub fn end_remote_cursor(&mut self) {
        self.virtual_cursor = None;
        self.virtual_bounds = None;
        self.remote_cursor_offset = (0.0, 0.0);
        self.remote_device = None;
        self.remote_node = None;
        self.last_cursor_emit_ms = 0;
        self.remote_return_direction = None;
        self.remote_entry_direction = None;
        self.remote_inside = false;
        self.remote_outward_events = 0;
        self.remote_outward_direction = None;
        self.last_escape_ms = 0;
        self.escape_down = false;
        self.controlling_remote = false;
        self.remote_input_released = false;
        self.remote_paused = false;
        self.last_injected = None;
        self.set_absolute_pointer(false);
        self.reset_absolute_edge();
    }

    pub fn is_recent_injection(&self, event: &nc_core::InputEvent) -> bool {
        let Some((last, at)) = &self.last_injected else {
            return false;
        };
        if at.elapsed() > Duration::from_millis(32) {
            return false;
        }
        match (last, event) {
            (
                nc_core::InputEvent::Key {
                    state: last_state,
                    key_code: last_key,
                    ..
                },
                nc_core::InputEvent::Key {
                    state, key_code, ..
                },
            ) => last_state == state && last_key == key_code,
            (
                nc_core::InputEvent::MouseMove {
                    x: last_x,
                    y: last_y,
                    ..
                },
                nc_core::InputEvent::MouseMove { x, y, .. },
            ) => last_x.abs_diff(*x) <= 1 && last_y.abs_diff(*y) <= 1,
            (
                nc_core::InputEvent::MouseButton {
                    state: last_state,
                    button: last_button,
                    x: last_x,
                    y: last_y,
                    ..
                },
                nc_core::InputEvent::MouseButton {
                    state,
                    button,
                    x,
                    y,
                    ..
                },
            ) => {
                last_state == state
                    && last_button == button
                    && last_x.abs_diff(*x) <= 1
                    && last_y.abs_diff(*y) <= 1
            }
            (
                nc_core::InputEvent::MouseWheel {
                    delta_x: last_x,
                    delta_y: last_y,
                    x: last_position_x,
                    y: last_position_y,
                    ..
                },
                nc_core::InputEvent::MouseWheel {
                    delta_x,
                    delta_y,
                    x,
                    y,
                    ..
                },
            ) => {
                last_x == delta_x
                    && last_y == delta_y
                    && last_position_x.abs_diff(*x) <= 1
                    && last_position_y.abs_diff(*y) <= 1
            }
            _ => false,
        }
    }

    pub fn remote_cursor_fraction(&self) -> Option<(DeviceId, f64, f64)> {
        let device_id = self.remote_device?;
        let (x, y) = self.virtual_cursor?;
        let (width, height) = self.virtual_bounds?;
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        Some((
            device_id,
            (x / width).clamp(0.0, 1.0),
            (y / height).clamp(0.0, 1.0),
        ))
    }

    pub fn set_relative_mode(&self, enabled: bool) {
        self.relative_mode.store(enabled, Ordering::SeqCst);
        if let Some(capture) = &self.capture {
            capture.set_relative_mode(enabled);
        }
    }

    pub fn set_suppressed(&self, suppressed: bool) {
        self.suppressed.store(suppressed, Ordering::SeqCst);
        if let Some(capture) = &self.capture {
            capture.set_suppressed(suppressed);
        }
    }

    pub fn is_suppressed(&self) -> bool {
        self.suppressed.load(Ordering::SeqCst)
    }

    pub fn pause_remote_control(&mut self) -> bool {
        if !self.controlling_remote || self.remote_paused {
            return false;
        }
        self.remote_paused = true;
        self.set_relative_mode(false);
        self.set_absolute_pointer(false);
        self.set_suppressed(false);
        true
    }

    pub fn resume_remote_control(&mut self) -> bool {
        if !self.remote_paused {
            return false;
        }
        self.remote_paused = false;
        true
    }

    pub fn inject(
        &mut self,
        event: &nc_core::InputEvent,
        use_service: bool,
    ) -> nc_core::Result<()> {
        if !matches!(event, nc_core::InputEvent::ReleaseAll) {
            self.last_injected = Some((event.clone(), Instant::now()));
        }
        #[cfg(windows)]
        if use_service && crate::input_service::inject(event) {
            return Ok(());
        }
        #[cfg(not(windows))]
        let _ = use_service;
        let result = match &mut self.injector {
            Some(injector) => injector.inject(event),
            None => {
                #[cfg(target_os = "macos")]
                if !matches!(event, nc_core::InputEvent::ReleaseAll) {
                    static WARNED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !WARNED.swap(true, Ordering::SeqCst) {
                        tracing::warn!("输入注入器未创建（缺少辅助功能授权），远端输入事件被丢弃");
                    }
                }
                Ok(())
            }
        };
        if result.is_err() {
            self.last_injected = None;
        }
        result
    }

    pub fn release_all(&mut self) {
        self.last_injected = None;
        #[cfg(windows)]
        let _ = crate::input_service::inject(&nc_core::InputEvent::ReleaseAll);
        if let Some(injector) = &mut self.injector {
            let _ = injector.release_all();
        }
    }
}

const INPUT_CAPTURE_INITIAL_DELAY: Duration = Duration::from_millis(500);
const INPUT_CAPTURE_RETRY_MIN: Duration = Duration::from_secs(2);
const INPUT_CAPTURE_RETRY_MAX: Duration = Duration::from_secs(10);

struct InputCapturePart {
    capture: Box<dyn InputCapture>,
    receiver: crossbeam_channel::Receiver<nc_input::CapturedInput>,
}

fn create_capture_part() -> nc_core::Result<InputCapturePart> {
    let mut capture = nc_input::create_input_capture()?;
    let (sink, receiver) = crossbeam_channel::unbounded();
    capture.start(sink)?;
    Ok(InputCapturePart { capture, receiver })
}

fn create_injector() -> nc_core::Result<Box<dyn InputInjector>> {
    nc_input::create_input_injector()
}

pub fn spawn_input_capture(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut delay = INPUT_CAPTURE_RETRY_MIN;
        tokio::time::sleep(INPUT_CAPTURE_INITIAL_DELAY).await;
        loop {
            if state.quitting.load(Ordering::SeqCst) {
                return;
            }
            let (has_capture, has_injector) = {
                let input = state.input.lock();
                (input.capture.is_some(), input.injector.is_some())
            };
            if has_capture && has_injector {
                return;
            }
            if !input_capture_allowed() {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(INPUT_CAPTURE_RETRY_MAX);
                continue;
            }
            if !has_injector {
                match tokio::task::spawn_blocking(create_injector).await {
                    Ok(Ok(injector)) => {
                        let mut input = state.input.lock();
                        if input.injector.is_none() {
                            input.injector = Some(injector);
                            tracing::info!("输入注入器已就绪");
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "输入注入器创建失败，稍后重试");
                    }
                    Err(error) => {
                        tracing::warn!(%error, "输入注入器创建任务异常结束，稍后重试");
                    }
                }
            }
            if !has_capture {
                match tokio::task::spawn_blocking(create_capture_part).await {
                    Ok(Ok(part)) => {
                        let mut input = state.input.lock();
                        if input.capture.is_none() {
                            input.install_capture_part(part, state.events.clone());
                            tracing::info!("输入捕获已启动");
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "输入捕获初始化失败，稍后重试");
                    }
                    Err(error) => {
                        tracing::warn!(%error, "输入捕获初始化任务异常结束，稍后重试");
                    }
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(INPUT_CAPTURE_RETRY_MAX);
        }
    });
}

#[cfg(target_os = "macos")]
fn input_capture_allowed() -> bool {
    nc_platform::permission_report()
        .map(|report| report.get(PermissionKind::Accessibility) == Some(true))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn input_capture_allowed() -> bool {
    true
}

pub(crate) fn stage_pointer_to_remote(
    stage_bounds: (f64, f64, f64, f64),
    stream_aspect: f64,
    remote_bounds: (f64, f64),
    x: f64,
    y: f64,
) -> Option<(f64, f64)> {
    let (stage_x, stage_y, stage_width, stage_height) = stage_bounds;
    let (remote_width, remote_height) = remote_bounds;
    if stage_width <= 0.0
        || stage_height <= 0.0
        || stream_aspect <= 0.0
        || remote_width <= 0.0
        || remote_height <= 0.0
    {
        return None;
    }
    let content_width = stage_width.min(stage_height * stream_aspect);
    let content_height = content_width / stream_aspect;
    let content_x = stage_x + (stage_width - content_width) / 2.0;
    let content_y = stage_y + (stage_height - content_height) / 2.0;
    let fraction_x = ((x - content_x) / content_width).clamp(0.0, 1.0);
    let fraction_y = ((y - content_y) / content_height).clamp(0.0, 1.0);
    Some((fraction_x * remote_width, fraction_y * remote_height))
}

pub(crate) fn remote_point_to_stage(
    stage_bounds: (f64, f64, f64, f64),
    stream_aspect: f64,
    remote_bounds: (f64, f64),
    point: (f64, f64),
) -> Option<(f64, f64)> {
    let (stage_x, stage_y, stage_width, stage_height) = stage_bounds;
    let (remote_width, remote_height) = remote_bounds;
    if stage_width <= 0.0
        || stage_height <= 0.0
        || stream_aspect <= 0.0
        || remote_width <= 0.0
        || remote_height <= 0.0
    {
        return None;
    }
    let content_width = stage_width.min(stage_height * stream_aspect);
    let content_height = content_width / stream_aspect;
    let content_x = stage_x + (stage_width - content_width) / 2.0;
    let content_y = stage_y + (stage_height - content_height) / 2.0;
    let fraction_x = (point.0 / remote_width).clamp(0.0, 1.0);
    let fraction_y = (point.1 / remote_height).clamp(0.0, 1.0);
    Some((
        content_x + fraction_x * content_width,
        content_y + fraction_y * content_height,
    ))
}

fn spawn_input_forwarder(
    receiver: crossbeam_channel::Receiver<nc_input::CapturedInput>,
    active: Arc<AtomicBool>,
    events: UnboundedSender<AppEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut pending_relative: Option<(nc_input::CapturedInput, Instant)> = None;
        let mut pending_absolute: Option<nc_input::CapturedInput> = None;
        let mut last_absolute_sent = Instant::now() - ABSOLUTE_MOVE_COALESCE;
        loop {
            match receiver.recv_timeout(Duration::from_millis(2)) {
                Ok(item) => {
                    if !active.load(Ordering::SeqCst) {
                        pending_relative = None;
                        pending_absolute = None;
                        continue;
                    }
                    if item.raw_relative
                        && matches!(item.event, nc_core::InputEvent::MouseMove { .. })
                    {
                        if let Some(pending) = pending_absolute.take() {
                            if events.send(AppEvent::Input(pending)).is_err() {
                                break;
                            }
                            last_absolute_sent = Instant::now();
                        }
                        if let Some((pending, _)) = pending_relative.as_mut() {
                            merge_relative_move(pending, item);
                        } else {
                            pending_relative = Some((item, Instant::now()));
                        }
                        let ready = pending_relative
                            .as_ref()
                            .is_some_and(|(_, since)| since.elapsed() >= RELATIVE_MOVE_COALESCE);
                        if ready {
                            if let Some((pending, _)) = pending_relative.take() {
                                if events.send(AppEvent::Input(pending)).is_err() {
                                    break;
                                }
                            }
                        }
                        continue;
                    }
                    if let Some((pending, _)) = pending_relative.take() {
                        if events.send(AppEvent::Input(pending)).is_err() {
                            break;
                        }
                    }
                    if matches!(item.event, nc_core::InputEvent::MouseMove { .. }) {
                        pending_absolute = Some(item);
                        if last_absolute_sent.elapsed() >= ABSOLUTE_MOVE_COALESCE {
                            if let Some(pending) = pending_absolute.take() {
                                if events.send(AppEvent::Input(pending)).is_err() {
                                    break;
                                }
                                last_absolute_sent = Instant::now();
                            }
                        }
                        continue;
                    }
                    if let Some(pending) = pending_absolute.take() {
                        if events.send(AppEvent::Input(pending)).is_err() {
                            break;
                        }
                        last_absolute_sent = Instant::now();
                    }
                    if events.send(AppEvent::Input(item)).is_err() {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if !active.load(Ordering::SeqCst) {
                        pending_relative = None;
                        pending_absolute = None;
                        continue;
                    }
                    let relative_ready = pending_relative
                        .as_ref()
                        .is_some_and(|(_, since)| since.elapsed() >= RELATIVE_MOVE_COALESCE);
                    if relative_ready {
                        if let Some((pending, _)) = pending_relative.take() {
                            if events.send(AppEvent::Input(pending)).is_err() {
                                break;
                            }
                        }
                    }
                    if pending_absolute.is_some()
                        && last_absolute_sent.elapsed() >= ABSOLUTE_MOVE_COALESCE
                    {
                        if let Some(pending) = pending_absolute.take() {
                            if events.send(AppEvent::Input(pending)).is_err() {
                                break;
                            }
                            last_absolute_sent = Instant::now();
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    })
}

fn merge_relative_move(pending: &mut nc_input::CapturedInput, next: nc_input::CapturedInput) {
    if let (
        nc_core::InputEvent::MouseMove {
            delta_x,
            delta_y,
            modifiers,
            ..
        },
        nc_core::InputEvent::MouseMove {
            delta_x: next_x,
            delta_y: next_y,
            modifiers: next_modifiers,
            ..
        },
    ) = (&mut pending.event, next.event)
    {
        *delta_x = delta_x.saturating_add(next_x);
        *delta_y = delta_y.saturating_add(next_y);
        *modifiers = next_modifiers;
        pending.timestamp_us = next.timestamp_us;
    }
}

impl Drop for InputRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

pub enum TransferEntry {
    Sending {
        device_id: DeviceId,
        sender: FileSender,
        view: TransferView,
    },
    Receiving {
        device_id: DeviceId,
        transfer_id: FileTransferId,
        receiver: Option<FileReceiver>,
        view: TransferView,
    },
}

impl TransferEntry {
    pub fn view(&self) -> &TransferView {
        match self {
            Self::Sending { view, .. } | Self::Receiving { view, .. } => view,
        }
    }

    pub fn view_mut(&mut self) -> &mut TransferView {
        match self {
            Self::Sending { view, .. } | Self::Receiving { view, .. } => view,
        }
    }
}

#[derive(Default)]
pub struct TransferState {
    pub next_id: u64,
    pub entries: BTreeMap<u64, TransferEntry>,
}

impl TransferState {
    pub fn next_id(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    pub fn views(&self) -> Vec<TransferView> {
        self.entries
            .values()
            .map(|entry| entry.view().clone())
            .collect()
    }

    pub fn remove_finished(&mut self) {
        self.entries.retain(|_, entry| {
            !matches!(
                entry.view().state,
                crate::ipc::TransferState::Done
                    | crate::ipc::TransferState::Failed
                    | crate::ipc::TransferState::Cancelled
            )
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderClass {
    Software,
    Integrated,
    Discrete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

fn gpu_vendor(name: &str) -> GpuVendor {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("nvidia") || normalized.contains("nvenc") {
        GpuVendor::Nvidia
    } else if normalized.contains("amd") || normalized.contains("amf") {
        GpuVendor::Amd
    } else if normalized.contains("intel") || normalized.contains("quick sync") {
        GpuVendor::Intel
    } else {
        GpuVendor::Unknown
    }
}

fn is_basic_render_driver(gpu: &nc_platform::GpuInfo) -> bool {
    gpu.name.to_ascii_lowercase().contains("basic render")
}

fn classify_encoder(hardware: bool, backend: &str, gpu: &nc_platform::GpuInfo) -> EncoderClass {
    if !hardware {
        return EncoderClass::Software;
    }
    if is_basic_render_driver(gpu) {
        return EncoderClass::Software;
    }
    let backend_vendor = gpu_vendor(backend);
    let gpu_vendor = gpu_vendor(&gpu.name);
    if backend_vendor == GpuVendor::Unknown
        || gpu_vendor == GpuVendor::Unknown
        || backend_vendor == gpu_vendor
    {
        if gpu.integrated {
            return EncoderClass::Integrated;
        }
        return EncoderClass::Discrete;
    }
    EncoderClass::Integrated
}

fn sustained_high_refresh(probe: &nc_codec::SustainedEncoderProbe) -> bool {
    probe.fps >= 108.0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoCapabilities {
    pub class: EncoderClass,
    pub hardware_encode: bool,
    pub basic_render_driver: bool,
    pub high_refresh: bool,
    pub preferred_encoder_vendor: Option<nc_codec::EncoderVendor>,
}

impl VideoCapabilities {
    pub fn detect() -> Self {
        let gpu = nc_platform::gpu_info();
        let preferred_encoder_vendor = if gpu.integrated {
            None
        } else {
            nc_codec::EncoderVendor::from_name(&gpu.name)
        };
        let basic_render_driver = is_basic_render_driver(&gpu);
        let probe = nc_codec::probe_video_encoder_with_preference(preferred_encoder_vendor);
        let class = classify_encoder(probe.hardware, &probe.backend, &gpu);
        let hardware_encode = probe.hardware && class != EncoderClass::Software;
        let sustained = if hardware_encode {
            nc_codec::probe_video_encoder_sustained(
                nc_codec::EncoderConfig {
                    width: 1920,
                    height: 1080,
                    fps: 120,
                    bitrate_kbps: 59_500,
                    keyframe_interval: 240,
                    preset: nc_codec::EncoderPreset::Fast,
                },
                preferred_encoder_vendor,
            )
        } else {
            None
        };
        let high_refresh = sustained.as_ref().is_some_and(sustained_high_refresh);
        tracing::info!(
            hardware = probe.hardware,
            backend = %probe.backend,
            gpu = %gpu.name,
            integrated = gpu.integrated,
            basic_render = basic_render_driver,
            preferred_vendor = ?preferred_encoder_vendor,
            class = ?class,
            high_refresh,
            sustained_fps = ?sustained.as_ref().map(|probe| probe.fps),
            sustained_backend = ?sustained.as_ref().map(|probe| probe.backend.as_str()),
            "视频编码能力探测完成"
        );
        Self {
            class,
            hardware_encode,
            basic_render_driver,
            high_refresh,
            preferred_encoder_vendor,
        }
    }

    pub const fn limits(&self) -> (u32, u32, u32) {
        #[cfg(target_os = "macos")]
        if self.hardware_encode && cfg!(target_arch = "aarch64") {
            return (3840, 2160, if self.high_refresh { 120 } else { 60 });
        }
        match self.class {
            EncoderClass::Software if self.basic_render_driver => (1920, 1080, 60),
            EncoderClass::Software => (1280, 720, 30),
            EncoderClass::Integrated => (1920, 1080, if self.high_refresh { 120 } else { 60 }),
            EncoderClass::Discrete => (3840, 2160, 144),
        }
    }

    pub fn limit_target(&self, target: VideoTarget) -> VideoTarget {
        let (max_width, max_height, max_fps) = self.limits();
        target.clamped_to(max_width, max_height, max_fps)
    }
}

#[derive(Debug, Clone, Default)]
pub struct AdaptiveState {
    pub level: u8,
    pub bad_ticks: u32,
    pub good_ticks: u32,
}

#[derive(Debug, Default)]
pub struct SharedConfigSyncState {
    pub acks: HashMap<DeviceId, u64>,
}

pub struct UpdateRuntime {
    pub manager: UpdateManager,
    pub incoming: Option<UpdateReceiver>,
    pub incoming_manifest: Option<UpdateManifest>,
    pub staged_path: Option<std::path::PathBuf>,
    pub push: Option<UpdatePush>,
    pub status: UpdateStatusView,
}

pub struct UpdatePush {
    pub manifest: UpdateManifest,
    pub peers: Vec<UpdatePeerPush>,
}

pub struct UpdatePeerPush {
    pub device_id: DeviceId,
    pub device_name: String,
    pub state: crate::ipc::UpdatePeerState,
    pub progress_percent: u8,
    pub message: Option<String>,
}

impl UpdateRuntime {
    pub fn new(manager: UpdateManager) -> Self {
        if let Some(status) = manager.read_applied_status() {
            if status.contains("\"status\":\"failed\"") {
                tracing::warn!(%status, "上次更新替换未完成");
            } else {
                tracing::info!(%status, "上次更新结果");
            }
        }
        let rollback_versions = manager
            .list_rollback()
            .into_iter()
            .map(|entry| entry.version)
            .collect();
        let staged = manager.load_staged();
        let (incoming_manifest, staged_path, status) = match staged {
            Some(staged) => (
                Some(staged.manifest.clone()),
                Some(staged.path),
                UpdateStatusView {
                    state: crate::ipc::UpdateState::Staged,
                    version: Some(staged.manifest.version),
                    kind: Some(staged.manifest.kind),
                    progress_percent: 100,
                    message: None,
                    rollback_versions,
                    peers: Vec::new(),
                },
            ),
            None => (
                None,
                None,
                UpdateStatusView {
                    rollback_versions,
                    ..UpdateStatusView::default()
                },
            ),
        };
        Self {
            manager,
            incoming: None,
            incoming_manifest,
            staged_path,
            push: None,
            status,
        }
    }

    pub fn refresh_rollback_versions(&mut self) {
        self.status.rollback_versions = self
            .manager
            .list_rollback()
            .into_iter()
            .map(|entry| entry.version)
            .collect();
    }
}

#[derive(Debug, Clone)]
pub enum BrokerEvent {
    Enter { count: u32, x: i32, y: i32 },
    Over { x: i32, y: i32 },
    Leave,
    Drop { paths: Vec<String>, x: i32, y: i32 },
    Error { message: String },
    Disconnected { reason: String },
    Unavailable { reason: String },
}

pub enum AppEvent {
    Action(UiAction),
    Discovery(nc_discovery::DiscoveryEvent),
    Control {
        connection_id: u64,
        event: nc_control::ControlEvent,
    },
    Session(nc_session::SessionEvent),
    Input(nc_input::CapturedInput),
    Media {
        connection_id: u64,
        event: MediaEvent,
    },
    MetricsTick,
    FileDropBroker(BrokerEvent),
    SecureDesktopTick,
    PermissionTick,
    WebCodecs(WebCodecsProbe),
    PresentStats {
        decode_ms: f64,
        present_ms: f64,
        frames: u32,
        presented_frames: u32,
        dropped_frames: u32,
        timestamp_us: u64,
    },
    ReconnectWake {
        generation: u64,
        attempt: u32,
        max: u32,
    },
    TransferTick {
        id: u64,
    },
    FileDropHint,
    EdgeTick,
    ClipboardTick,
    Clipboard(nc_clipboard::ClipboardPayload),
    ClipboardError(String),
}

pub struct AppState {
    pub app: AppHandle,
    pub paths: AppPaths,
    pub test_mode: bool,
    pub started: Instant,
    pub settings: RwLock<AppSettings>,
    pub identity: Arc<Identity>,
    pub certificate: Arc<DeviceCertificate>,
    pub trust: Arc<std::sync::RwLock<TrustStore>>,
    pub layout: RwLock<Layout>,
    pub monitors: RwLock<Vec<nc_platform::MonitorInfo>>,
    pub session: Mutex<SessionMachine>,
    pub snapshot: RwLock<SessionSnapshot>,
    pub devices: RwLock<HashMap<DeviceId, nc_discovery::DiscoveredDevice>>,
    pub discovery: Mutex<Option<DiscoveryService>>,
    pub notifications: Mutex<NotificationCenter>,
    pub metrics: Arc<MetricsRegistry>,
    pub clock_offsets: Mutex<HashMap<DeviceId, std::collections::VecDeque<i64>>>,

    pub bridge: Arc<MediaBridge>,
    pub bridge_info: MediaBridgeInfo,
    pub media: MediaRuntime,
    pub last_stream_request: Mutex<Option<StreamRequest>>,
    pub input: Mutex<InputRuntime>,
    pub transfers: Mutex<TransferState>,
    pub shared_sync: Mutex<SharedConfigSyncState>,
    pub update: Mutex<UpdateRuntime>,
    pub links: Mutex<HashMap<DeviceId, ActiveControl>>,
    pub connecting_links: Mutex<HashSet<DeviceId>>,
    pub session_peer: RwLock<Option<DeviceId>>,
    pub session_role: RwLock<Option<SessionRole>>,
    pub session_peer_mode: RwLock<Option<nc_core::Mode>>,
    pub group_host: RwLock<Option<DeviceId>>,
    pub pending_session: RwLock<Option<DeviceId>>,
    pub pending_remote_entry: Mutex<Option<PendingRemoteEntry>>,
    pub switching_target: AtomicBool,
    pub pending_session_started: Mutex<Option<Instant>>,
    pub pending_session_retries: AtomicU8,
    pub pending_takeover: Mutex<Option<PendingTakeover>>,
    pub registered_shortcuts: parking_lot::Mutex<Vec<tauri_plugin_global_shortcut::Shortcut>>,
    pub aux_controls: Mutex<HashMap<u64, AuxControl>>,
    pub pairing: Mutex<Option<PairingState>>,
    pub actions: UnboundedSender<UiAction>,
    pub events: UnboundedSender<AppEvent>,
    pub tray: RwLock<Option<TrayHandle>>,
    pub quitting: Arc<AtomicBool>,
    pub webcodecs: RwLock<WebCodecsProbe>,
    pub video_capabilities: VideoCapabilities,
    pub device_info: RwLock<DeviceInfo>,
    pub control_listener_port: u16,
    pub media_port: u16,
    pub session_generation: AtomicU64,
    pub osd_generation: AtomicU64,
    pub drop_osd_generation: AtomicU64,
    pub drop_osd_visible: AtomicBool,
    pub file_drop_hover: parking_lot::Mutex<Option<String>>,
    pub file_drop_cards: parking_lot::Mutex<Option<FileDropCardsView>>,
    pub control_osd_generation: AtomicU64,
    pub control_osd_last_input_ms: AtomicU64,
    pub control_osd_active: AtomicBool,
    pub edge_halo: Mutex<Option<EdgeHaloView>>,
    pub reconnect_task: Mutex<Option<JoinHandle<()>>>,
    pub clipboard_last_hash: RwLock<Option<[u8; 32]>>,
    pub clipboard: Mutex<Option<nc_clipboard::ClipboardWatcher>>,
    pub pending_connect: RwLock<Option<DeviceId>>,
    pub permissions: RwLock<nc_core::PermissionReport>,
    pub first_frame_wait: Mutex<Option<(StreamId, Instant)>>,
    pub user_disconnect: AtomicBool,
    pub reconnect_attempt: AtomicU64,
    pub input_seq: AtomicU64,
    pub adaptive: Mutex<AdaptiveState>,
    pub secure_desktop: AtomicBool,
    pub secure_desktop_service: AtomicBool,
    pub main_pending_surface_render: AtomicBool,
    pub main_surface_ready: AtomicBool,
    pub stage_pending_surface_render: AtomicBool,
    pub stage_surface_ready: AtomicBool,
}

impl AppState {
    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot.read().clone()
    }

    pub fn generation(&self) -> u64 {
        self.session_generation.load(Ordering::SeqCst)
    }

    pub fn next_generation(&self) -> u64 {
        self.session_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn language(&self) -> nc_core::Language {
        self.settings.read().language
    }

    pub fn session_peer_id(&self) -> Option<DeviceId> {
        *self.session_peer.read()
    }

    pub fn record_clock_offset(&self, device: DeviceId, offset_us: i64) {
        const SAMPLES: usize = 8;
        let mut offsets = self.clock_offsets.lock();
        let samples = offsets.entry(device).or_default();
        samples.push_back(offset_us);
        while samples.len() > SAMPLES {
            samples.pop_front();
        }
    }

    pub fn clock_offset(&self, device: DeviceId) -> Option<i64> {
        self.clock_offsets
            .lock()
            .get(&device)
            .and_then(|samples| samples.iter().min().copied())
    }

    pub fn peer(&self) -> Option<DeviceInfo> {
        let device_id = self.session_peer_id()?;
        self.links
            .lock()
            .get(&device_id)
            .and_then(|control| control.peer_info())
    }

    pub fn link_peer(&self, device_id: DeviceId) -> Option<DeviceInfo> {
        self.links
            .lock()
            .get(&device_id)
            .and_then(|control| control.peer_info())
    }

    pub fn link_device(&self, connection_id: u64) -> Option<DeviceId> {
        self.links
            .lock()
            .iter()
            .find(|(_, link)| link.id == connection_id)
            .map(|(device_id, _)| *device_id)
    }

    pub fn device_views(&self) -> Vec<crate::ipc::DeviceView> {
        let trusted = self
            .trust
            .read()
            .map(|store| store.all())
            .unwrap_or_default();
        let connected: HashMap<DeviceId, DeviceInfo> = {
            let links = self.links.lock();
            links
                .iter()
                .filter(|(_, link)| link.connected.load(Ordering::SeqCst))
                .filter_map(|(device, link)| link.peer_info().map(|peer| (*device, peer)))
                .collect()
        };
        let devices = self.devices.read();
        let mut views: Vec<_> = devices
            .values()
            .map(|device| crate::ipc::DeviceView {
                info: device.info.clone(),
                online: true,
                connected: connected.contains_key(&device.info.id),
                trusted: trusted
                    .iter()
                    .any(|entry| entry.device_id == device.info.id),
                address: Some(device.address.to_string()),
                last_seen_ms: device.last_seen_ms,
            })
            .collect();
        for entry in trusted {
            if views.iter().any(|view| view.info.id == entry.device_id) {
                continue;
            }
            if let Some(peer) = connected.get(&entry.device_id) {
                views.push(crate::ipc::DeviceView {
                    info: peer.clone(),
                    online: true,
                    connected: true,
                    trusted: true,
                    address: None,
                    last_seen_ms: crate::notifications::now_ms(),
                });
                continue;
            }
            views.push(crate::ipc::DeviceView {
                info: DeviceInfo {
                    id: entry.device_id,
                    name: entry.name,
                    platform: entry.platform,
                    capabilities: nc_core::Capabilities::default(),
                    control_port: 0,
                    media_port: 0,
                },
                online: false,
                connected: false,
                trusted: true,
                address: None,
                last_seen_ms: 0,
            });
        }
        views.sort_by(|left, right| left.info.name.cmp(&right.info.name));
        views
    }
}
