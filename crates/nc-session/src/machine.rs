use crate::effect::{SessionEffect, SessionNotice};
use crate::event::SessionEvent;
use nc_core::{
    AppSettings, ConnectionPhase, DeviceId, DisplayNodeId, InputOwner, Mode, SessionSnapshot,
    SessionState, UiState,
};
use nc_topology::Layout;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectPolicy {
    pub max_attempts: u32,
    pub delay_ms: u64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            delay_ms: 500,
        }
    }
}

impl ReconnectPolicy {
    pub const fn new(max_attempts: u32, delay_ms: u64) -> Self {
        Self {
            max_attempts,
            delay_ms,
        }
    }

    pub const fn retry_window_ms(self) -> u64 {
        self.max_attempts as u64 * self.delay_ms
    }
}

#[derive(Debug, Clone)]
pub struct SessionMachine {
    snapshot: SessionSnapshot,
    layout: Option<Layout>,
    pending_device: Option<DeviceId>,
    permission_lost: bool,
    media_running: bool,
    reconnect: ReconnectPolicy,
    stage_return_ui: UiState,
}

impl SessionMachine {
    pub fn new(mode: Mode) -> Self {
        let snapshot = SessionSnapshot {
            mode,
            ..SessionSnapshot::default()
        };
        Self {
            snapshot,
            layout: None,
            pending_device: None,
            permission_lost: false,
            media_running: false,
            reconnect: ReconnectPolicy::default(),
            stage_return_ui: UiState::Background,
        }
    }

    pub fn from_settings(settings: &AppSettings) -> Self {
        Self::new(settings.mode)
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot.clone()
    }

    pub fn state(&self) -> SessionState {
        self.snapshot.state
    }

    pub fn mode(&self) -> Mode {
        self.snapshot.mode
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = Some(layout);
    }

    pub fn reconnect_policy(&self) -> ReconnectPolicy {
        self.reconnect
    }

    pub fn set_reconnect_policy(&mut self, policy: ReconnectPolicy) {
        self.reconnect = policy;
    }

    pub fn handle(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
        self.handle_at(event, None)
    }

    pub fn handle_at(&mut self, event: SessionEvent, now_ms: Option<u64>) -> Vec<SessionEffect> {
        if self.snapshot.state == SessionState::Closed {
            return Vec::new();
        }

        let mut effects = Vec::new();
        match event {
            SessionEvent::StartDiscovery => self.start_discovery(&mut effects),
            SessionEvent::StopDiscovery => self.stop_discovery(&mut effects),
            SessionEvent::DevicesUpdated => {}
            SessionEvent::ConnectRequested { device, trusted } => {
                self.connect_requested(device, trusted, &mut effects);
            }
            SessionEvent::PairingStarted { device } => {
                self.pairing_started(device);
            }
            SessionEvent::SasRequired => {
                if self.snapshot.state == SessionState::Pairing {
                    self.snapshot.phase = ConnectionPhase::Authenticating;
                }
            }
            SessionEvent::PairingSucceeded { device } => {
                if self.snapshot.state == SessionState::Pairing {
                    self.pending_device = Some(device);
                    self.snapshot.state = SessionState::Connected;
                    self.snapshot.phase = ConnectionPhase::Ready;
                    self.snapshot.last_error = None;
                    effects.push(SessionEffect::Notice(SessionNotice::Paired));
                }
            }
            SessionEvent::PairingFailed { reason } => {
                if self.snapshot.state == SessionState::Pairing {
                    self.snapshot.state = SessionState::Idle;
                    self.snapshot.phase = ConnectionPhase::Offline;
                    self.snapshot.last_error = Some(reason);
                    self.pending_device = None;
                    effects.push(SessionEffect::Notice(SessionNotice::PairingFailed));
                }
            }
            SessionEvent::ConnectionEstablished { peer } => {
                self.connection_established(peer, now_ms, &mut effects);
            }
            SessionEvent::SwitchTarget { peer } => {
                self.switch_target(peer, now_ms, &mut effects);
            }
            SessionEvent::RemoteSessionClosed => {
                self.remote_session_closed(&mut effects);
            }
            SessionEvent::ConnectionLost { reason } => {
                self.connection_lost(reason, &mut effects);
            }
            SessionEvent::ReconnectTick { attempt, max } => {
                self.reconnect_tick(attempt, max, &mut effects);
            }
            SessionEvent::SetMode { mode } => self.apply_mode(mode, &mut effects),
            SessionEvent::ToggleMode => {
                let mode = match self.snapshot.mode {
                    Mode::Kvm => Mode::Aggregation,
                    Mode::Aggregation => Mode::Kvm,
                };
                self.apply_mode(mode, &mut effects);
            }
            SessionEvent::EdgeCrossed {
                to_device,
                to_node,
                direction: _,
            } => self.edge_crossed(to_device, to_node, &mut effects),
            SessionEvent::ReturnToLocal => self.return_to_local(&mut effects),
            SessionEvent::LocalDisplayNodeChanged { node } => self.local_display_node_changed(node),
            SessionEvent::RemoteDisplayNodeChanged { node } => {
                self.remote_display_node_changed(node)
            }
            SessionEvent::ReleaseAll => {
                self.set_input_owner(InputOwner::Local, &mut effects, false);
            }
            SessionEvent::StageSummon => {
                if self.snapshot.state == SessionState::Aggregating && !self.snapshot.stage_visible
                {
                    if let Some(device) = self.remote_display_device() {
                        self.snapshot.current_display_device = Some(device);
                        self.show_stage(&mut effects);
                    }
                }
            }
            SessionEvent::StageHide => {
                if self.snapshot.state == SessionState::Aggregating {
                    self.hide_stage(&mut effects);
                }
            }
            SessionEvent::UiStateChanged { ui_state } => {
                self.ui_state_changed(ui_state, &mut effects);
            }
            SessionEvent::PermissionLost => self.permission_lost(&mut effects),
            SessionEvent::PermissionRestored => self.permission_restored(&mut effects),
            SessionEvent::PeerInputOwner(owner) => {
                if self.snapshot.state.is_active() {
                    let previous = self.snapshot.input_owner.clone();
                    self.set_input_owner(owner, &mut effects, false);
                    let current = self.snapshot.input_owner.clone();
                    if previous.is_local() && !current.is_local() {
                        effects.push(SessionEffect::Notice(SessionNotice::ControlHandover));
                    } else if !previous.is_local() && current.is_local() {
                        effects.push(SessionEffect::Notice(SessionNotice::ControlReturned));
                    }
                }
            }
            SessionEvent::UserAbort => {
                self.set_input_owner(InputOwner::Local, &mut effects, true);
                let had_session = self.teardown(&mut effects);
                if had_session {
                    effects.push(SessionEffect::Notice(SessionNotice::Disconnected));
                }
            }
            SessionEvent::EmergencyRelease => {
                self.set_input_owner(InputOwner::Local, &mut effects, true);
                self.teardown(&mut effects);
                effects.push(SessionEffect::Notice(SessionNotice::EmergencyRelease));
            }
            SessionEvent::Shutdown => self.shutdown(&mut effects),
            SessionEvent::LayoutUpdated { layout } => {
                self.layout = Some(layout);
            }
        }
        effects
    }

    fn start_discovery(&mut self, effects: &mut Vec<SessionEffect>) {
        if self.pending_device.is_some() {
            return;
        }
        match self.snapshot.state {
            SessionState::Discovering => {}
            SessionState::Idle | SessionState::Error => {
                self.snapshot.state = SessionState::Discovering;
                self.snapshot.phase = ConnectionPhase::Discovering;
                self.snapshot.peer = None;
                self.snapshot.last_error = None;
                self.snapshot.current_display_device = None;
                self.snapshot.current_display_node = None;
                self.snapshot.connected_since_ms = None;
                self.permission_lost = false;
                effects.push(SessionEffect::StartDiscovery);
            }
            _ => {}
        }
    }

    fn stop_discovery(&mut self, effects: &mut Vec<SessionEffect>) {
        if self.snapshot.state == SessionState::Discovering {
            self.snapshot.state = SessionState::Idle;
            self.snapshot.phase = ConnectionPhase::Offline;
            effects.push(SessionEffect::StopDiscovery);
        }
    }

    fn connect_requested(
        &mut self,
        device: DeviceId,
        trusted: bool,
        effects: &mut Vec<SessionEffect>,
    ) {
        if self.pending_device.is_some() {
            return;
        }
        if !matches!(
            self.snapshot.state,
            SessionState::Idle | SessionState::Discovering | SessionState::Error
        ) {
            return;
        }
        self.pending_device = Some(device);
        self.snapshot.peer = None;
        self.snapshot.last_error = None;
        self.snapshot.connected_since_ms = None;
        self.snapshot.current_display_device = None;
        self.snapshot.current_display_node = None;
        self.snapshot.stage_visible = false;
        if self.snapshot.ui_state == UiState::Stage {
            self.snapshot.ui_state = UiState::Background;
        }

        if trusted {
            self.snapshot.state = SessionState::Idle;
            self.snapshot.phase = ConnectionPhase::TlsHandshake;
            effects.push(SessionEffect::Connect);
        } else {
            self.snapshot.state = SessionState::Pairing;
            self.snapshot.phase = ConnectionPhase::Pairing;
            effects.push(SessionEffect::StartPairing);
        }
    }

    fn pairing_started(&mut self, device: DeviceId) {
        if !matches!(
            self.snapshot.state,
            SessionState::Idle
                | SessionState::Discovering
                | SessionState::Error
                | SessionState::Pairing
        ) {
            return;
        }
        self.pending_device = Some(device);
        self.snapshot.state = SessionState::Pairing;
        self.snapshot.phase = ConnectionPhase::Pairing;
        self.snapshot.peer = None;
        self.snapshot.last_error = None;
    }

    fn connection_established(
        &mut self,
        peer: nc_core::DeviceInfo,
        now_ms: Option<u64>,
        effects: &mut Vec<SessionEffect>,
    ) {
        if !matches!(
            self.snapshot.state,
            SessionState::Idle
                | SessionState::Discovering
                | SessionState::Pairing
                | SessionState::Connected
                | SessionState::Error
                | SessionState::Reconnecting
        ) {
            return;
        }
        self.set_input_owner(InputOwner::Local, effects, false);
        self.snapshot.peer = Some(peer.clone());
        self.snapshot.phase = ConnectionPhase::Ready;
        self.pending_device = None;
        self.snapshot.last_error = None;
        self.snapshot.connected_since_ms = now_ms;
        self.snapshot.stage_visible = false;
        if self.snapshot.ui_state == UiState::Stage {
            self.snapshot.ui_state = UiState::Background;
        }

        if self.snapshot.mode == Mode::Aggregation {
            self.snapshot.state = SessionState::Aggregating;
            self.snapshot.current_display_device = None;
            self.snapshot.current_display_node = None;
        } else {
            self.snapshot.state = SessionState::Kvm;
            self.snapshot.current_display_device = None;
            self.snapshot.current_display_node = None;
        }

        effects.push(SessionEffect::Notice(SessionNotice::Connected));
    }

    fn switch_target(
        &mut self,
        peer: nc_core::DeviceInfo,
        now_ms: Option<u64>,
        effects: &mut Vec<SessionEffect>,
    ) {
        if !self.snapshot.state.is_active() {
            return;
        }
        let keep_stage = self.snapshot.mode == Mode::Aggregation && self.snapshot.stage_visible;
        self.set_input_owner(InputOwner::Local, effects, false);
        self.snapshot.peer = Some(peer.clone());
        self.snapshot.connected_since_ms = now_ms.or(self.snapshot.connected_since_ms);
        self.snapshot.last_error = None;
        self.snapshot.current_display_device = Some(peer.id);
        self.snapshot.current_display_node = self.primary_node_for_device(peer.id);

        if self.snapshot.mode == Mode::Aggregation {
            self.snapshot.state = SessionState::Aggregating;
            if keep_stage && !self.permission_lost {
                self.snapshot.stage_visible = true;
                self.snapshot.ui_state = UiState::Stage;
                self.set_input_owner(InputOwner::Remote { device_id: peer.id }, effects, false);
                self.start_media(Some(peer.id), effects);
            } else {
                self.snapshot.stage_visible = false;
                self.stop_media(effects);
            }
        } else {
            self.snapshot.state = SessionState::Kvm;
            self.snapshot.stage_visible = false;
            self.stop_media(effects);
        }
    }

    fn remote_session_closed(&mut self, effects: &mut Vec<SessionEffect>) {
        if !matches!(
            self.snapshot.state,
            SessionState::Connected | SessionState::Kvm | SessionState::Aggregating
        ) {
            return;
        }
        self.set_input_owner(InputOwner::Local, effects, false);
        self.stop_media(effects);
        self.hide_stage(effects);
        self.snapshot.state = SessionState::Idle;
        self.snapshot.phase = ConnectionPhase::Offline;
        self.snapshot.peer = None;
        self.snapshot.connected_since_ms = None;
        self.snapshot.current_display_device = None;
        self.snapshot.current_display_node = None;
        self.snapshot.stage_visible = false;
        self.media_running = false;
    }

    fn connection_lost(&mut self, reason: String, effects: &mut Vec<SessionEffect>) {
        if self.snapshot.state == SessionState::Reconnecting {
            self.snapshot.last_error = Some(reason);
            return;
        }
        if self.snapshot.state == SessionState::Pairing {
            self.snapshot.state = SessionState::Idle;
            self.snapshot.phase = ConnectionPhase::Offline;
            self.snapshot.last_error = Some(reason);
            self.pending_device = None;
            effects.push(SessionEffect::Notice(SessionNotice::PairingFailed));
            return;
        }
        if self.snapshot.state == SessionState::Idle && self.pending_device.is_some() {
            self.snapshot.state = SessionState::Error;
            self.snapshot.phase = ConnectionPhase::Failed;
            self.snapshot.last_error = Some(reason);
            self.pending_device = None;
            effects.push(SessionEffect::Notice(SessionNotice::Disconnected));
            return;
        }
        if !matches!(
            self.snapshot.state,
            SessionState::Connected | SessionState::Kvm | SessionState::Aggregating
        ) {
            return;
        }

        self.set_input_owner(InputOwner::Local, effects, false);
        self.stop_media(effects);
        self.hide_stage(effects);
        self.snapshot.state = SessionState::Reconnecting;
        self.snapshot.phase = ConnectionPhase::Reconnecting;
        self.snapshot.last_error = Some(reason);
        self.snapshot.connected_since_ms = None;
        self.snapshot.current_display_device = None;
        self.snapshot.current_display_node = None;
        effects.push(SessionEffect::Notice(SessionNotice::Reconnecting));
    }

    fn reconnect_tick(&mut self, attempt: u32, max: u32, effects: &mut Vec<SessionEffect>) {
        if self.snapshot.state != SessionState::Reconnecting {
            return;
        }
        let effective_max = if max == 0 {
            self.reconnect.max_attempts
        } else {
            max
        };
        if effective_max == 0 || attempt > effective_max {
            self.snapshot.state = SessionState::Error;
            self.snapshot.phase = ConnectionPhase::Failed;
            self.snapshot.last_error = Some(format!(
                "重连失败（已尝试 {attempt} 次，上限 {effective_max} 次）"
            ));
            effects.push(SessionEffect::Notice(SessionNotice::ReconnectFailed));
        } else {
            effects.push(SessionEffect::Connect);
        }
    }

    fn apply_mode(&mut self, mode: Mode, effects: &mut Vec<SessionEffect>) {
        if self.snapshot.mode == mode {
            return;
        }
        let previous_state = self.snapshot.state;
        self.snapshot.mode = mode;
        effects.push(SessionEffect::PersistMode(mode));
        if previous_state.is_active() {
            effects.push(SessionEffect::Notice(SessionNotice::ModeChanged));
        }

        match (previous_state, mode) {
            (SessionState::Kvm, Mode::Aggregation) => {
                self.set_input_owner(InputOwner::Local, effects, false);
                self.snapshot.state = SessionState::Aggregating;
                self.snapshot.current_display_device = None;
                self.snapshot.current_display_node = None;
            }
            (SessionState::Aggregating, Mode::Kvm) => {
                self.hide_stage(effects);
                self.snapshot.state = SessionState::Kvm;
                self.snapshot.current_display_device = None;
                self.snapshot.current_display_node = None;
            }
            _ => {}
        }
    }

    fn edge_crossed(
        &mut self,
        to_device: DeviceId,
        to_node: DisplayNodeId,
        effects: &mut Vec<SessionEffect>,
    ) {
        match self.snapshot.state {
            SessionState::Kvm => {
                self.snapshot.current_display_device = Some(to_device);
                self.snapshot.current_display_node = Some(to_node);
                let owner = if self.is_local_device(to_device) {
                    InputOwner::Local
                } else {
                    InputOwner::Remote {
                        device_id: to_device,
                    }
                };
                self.set_input_owner(owner, effects, false);
            }
            SessionState::Aggregating => {
                let is_local = self.is_local_device(to_device);
                let display_changed = self.snapshot.current_display_device != Some(to_device)
                    || self.snapshot.current_display_node.as_ref() != Some(&to_node);
                self.snapshot.current_display_device = Some(to_device);
                self.snapshot.current_display_node = Some(to_node);

                if is_local {
                    self.set_input_owner(InputOwner::Local, effects, false);
                    if self.snapshot.stage_visible {
                        self.hide_stage(effects);
                    }
                } else {
                    self.set_input_owner(
                        InputOwner::Remote {
                            device_id: to_device,
                        },
                        effects,
                        false,
                    );
                    if !self.snapshot.stage_visible {
                        self.show_stage(effects);
                    } else if display_changed && !self.permission_lost {
                        self.start_media(Some(to_device), effects);
                    }
                }
            }
            _ => {}
        }
    }

    fn return_to_local(&mut self, effects: &mut Vec<SessionEffect>) {
        match self.snapshot.state {
            SessionState::Kvm => {
                self.set_input_owner(InputOwner::Local, effects, false);
                let local = self.local_device();
                self.snapshot.current_display_device = local;
                self.snapshot.current_display_node = self.local_display_node();
            }
            SessionState::Aggregating => {
                self.set_input_owner(InputOwner::Local, effects, false);
                let local = self.local_device();
                self.snapshot.current_display_device = local;
                self.snapshot.current_display_node = self.local_display_node();
                if self.snapshot.stage_visible {
                    self.hide_stage(effects);
                }
            }
            _ => {}
        }
    }

    fn show_stage(&mut self, effects: &mut Vec<SessionEffect>) {
        let Some(device) = self.remote_display_device() else {
            return;
        };
        if self.snapshot.ui_state != UiState::Stage {
            self.stage_return_ui = self.snapshot.ui_state;
        }
        self.snapshot.current_display_device = Some(device);
        self.snapshot.current_display_node = self
            .snapshot
            .current_display_node
            .clone()
            .filter(|node| self.node_belongs_to(node, device))
            .or_else(|| self.primary_node_for_device(device));
        self.set_input_owner(InputOwner::Remote { device_id: device }, effects, false);
        effects.push(SessionEffect::ShowStage);
        effects.push(SessionEffect::SwitchWindowState(UiState::Stage));
        self.snapshot.stage_visible = true;
        self.snapshot.ui_state = UiState::Stage;
        self.sync_media_for_display(effects);
    }

    fn hide_stage(&mut self, effects: &mut Vec<SessionEffect>) {
        self.set_input_owner(InputOwner::Local, effects, false);
        if !self.snapshot.stage_visible {
            return;
        }
        let return_ui = self.stage_return_ui;
        effects.push(SessionEffect::HideStage);
        if return_ui == UiState::ControlPanel {
            effects.push(SessionEffect::SwitchWindowState(UiState::ControlPanel));
        } else {
            effects.push(SessionEffect::BackToBackground);
        }
        self.stop_media(effects);
        self.snapshot.stage_visible = false;
        self.snapshot.ui_state = return_ui;
        self.stage_return_ui = UiState::Background;
    }

    fn ui_state_changed(&mut self, ui_state: UiState, effects: &mut Vec<SessionEffect>) {
        if ui_state == self.snapshot.ui_state {
            return;
        }
        match ui_state {
            UiState::Stage => {
                if self.snapshot.state != SessionState::Aggregating {
                    return;
                }
                let Some(device) = self.remote_display_device() else {
                    return;
                };
                self.snapshot.current_display_device = Some(device);
                self.snapshot.current_display_node = self
                    .snapshot
                    .current_display_node
                    .clone()
                    .filter(|node| self.node_belongs_to(node, device))
                    .or_else(|| self.primary_node_for_device(device));
                self.set_input_owner(InputOwner::Remote { device_id: device }, effects, false);
                let was_visible = self.snapshot.stage_visible;
                self.snapshot.ui_state = UiState::Stage;
                self.snapshot.stage_visible = true;
                if !was_visible {
                    effects.push(SessionEffect::SwitchWindowState(UiState::Stage));
                    self.sync_media_for_display(effects);
                }
            }
            UiState::Background | UiState::ControlPanel => {
                self.set_input_owner(InputOwner::Local, effects, false);
                if self.snapshot.stage_visible || self.snapshot.ui_state == UiState::Stage {
                    self.snapshot.stage_visible = false;
                    self.stop_media(effects);
                }
                self.snapshot.ui_state = ui_state;
            }
        }
    }

    fn permission_lost(&mut self, effects: &mut Vec<SessionEffect>) {
        if !self.snapshot.state.is_active() || self.permission_lost {
            return;
        }
        self.permission_lost = true;
        self.set_input_owner(InputOwner::Local, effects, false);
        self.hide_stage(effects);
        self.stop_media(effects);
        self.snapshot.last_error = Some("系统权限丢失".into());
        effects.push(SessionEffect::Notice(SessionNotice::PermissionLost));
    }

    fn permission_restored(&mut self, effects: &mut Vec<SessionEffect>) {
        if !self.permission_lost {
            return;
        }
        self.permission_lost = false;
        self.snapshot.last_error = None;
        if self.snapshot.state == SessionState::Aggregating && self.snapshot.stage_visible {
            self.sync_media_for_display(effects);
        }
    }

    fn teardown(&mut self, effects: &mut Vec<SessionEffect>) -> bool {
        let previous = self.snapshot.state;
        let pending_connection = self.pending_device.is_some();
        let had_session = previous != SessionState::Idle || pending_connection;
        if previous == SessionState::Discovering {
            effects.push(SessionEffect::StopDiscovery);
        }
        if pending_connection
            || matches!(
                previous,
                SessionState::Connected
                    | SessionState::Kvm
                    | SessionState::Aggregating
                    | SessionState::Pairing
            )
        {
            effects.push(SessionEffect::Disconnect);
        }
        self.stop_media(effects);
        self.hide_stage(effects);

        self.snapshot.state = SessionState::Idle;
        self.snapshot.phase = ConnectionPhase::Offline;
        self.snapshot.peer = None;
        self.snapshot.stage_visible = false;
        self.snapshot.connected_since_ms = None;
        self.snapshot.last_error = None;
        self.snapshot.current_display_device = None;
        self.snapshot.current_display_node = None;
        self.pending_device = None;
        self.permission_lost = false;
        self.media_running = false;
        self.stage_return_ui = UiState::Background;
        had_session
    }

    fn shutdown(&mut self, effects: &mut Vec<SessionEffect>) {
        let previous = self.snapshot.state;
        let pending_connection = self.pending_device.is_some();
        self.set_input_owner(InputOwner::Local, effects, false);
        if previous == SessionState::Discovering {
            effects.push(SessionEffect::StopDiscovery);
        }
        if pending_connection
            || matches!(
                previous,
                SessionState::Connected
                    | SessionState::Kvm
                    | SessionState::Aggregating
                    | SessionState::Pairing
            )
        {
            effects.push(SessionEffect::Disconnect);
        }
        self.stop_media(effects);
        self.hide_stage(effects);

        self.snapshot.state = SessionState::Closed;
        self.snapshot.phase = ConnectionPhase::Offline;
        self.snapshot.peer = None;
        self.snapshot.stage_visible = false;
        self.snapshot.connected_since_ms = None;
        self.snapshot.last_error = None;
        self.snapshot.current_display_device = None;
        self.snapshot.current_display_node = None;
        self.snapshot.ui_state = UiState::Background;
        self.pending_device = None;
        self.permission_lost = false;
        self.media_running = false;
        self.stage_return_ui = UiState::Background;
    }

    fn sync_media_for_display(&mut self, effects: &mut Vec<SessionEffect>) {
        match self.snapshot.current_display_device {
            Some(device) if !self.is_local_device(device) && !self.permission_lost => {
                self.start_media(Some(device), effects);
            }
            _ => self.stop_media(effects),
        }
    }

    fn start_media(&mut self, target: Option<DeviceId>, effects: &mut Vec<SessionEffect>) {
        effects.push(SessionEffect::StartMedia { target });
        self.media_running = true;
    }

    fn stop_media(&mut self, effects: &mut Vec<SessionEffect>) {
        if self.media_running {
            effects.push(SessionEffect::StopMedia);
            self.media_running = false;
        }
    }

    fn local_device(&self) -> Option<DeviceId> {
        self.layout
            .as_ref()?
            .local_node()
            .map(|node| node.device_id)
    }

    fn local_display_node(&self) -> Option<DisplayNodeId> {
        self.layout
            .as_ref()?
            .local_node()
            .map(|node| node.id.clone())
    }

    fn local_display_node_changed(&mut self, node: DisplayNodeId) {
        if self.snapshot.input_owner != InputOwner::Local {
            return;
        }
        let Some(local) = self.local_device() else {
            return;
        };
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let Some(target) = layout.node(&node) else {
            return;
        };
        if target.device_id != local || !target.is_local || !layout.is_active(&node) {
            return;
        }
        self.snapshot.current_display_device = Some(local);
        self.snapshot.current_display_node = Some(node);
    }

    fn remote_display_node_changed(&mut self, node: DisplayNodeId) {
        if self.snapshot.input_owner.is_local() {
            return;
        }
        let Some(local) = self.local_device() else {
            return;
        };
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let Some(target) = layout.node(&node) else {
            return;
        };
        if target.device_id != local || !target.is_local || !layout.is_active(&node) {
            return;
        }
        self.snapshot.current_display_device = Some(local);
        self.snapshot.current_display_node = Some(node);
    }

    fn primary_node_for_device(&self, device: DeviceId) -> Option<DisplayNodeId> {
        let layout = self.layout.as_ref()?;
        layout
            .nodes
            .iter()
            .filter(|node| node.device_id == device && !node.is_local && layout.is_active(&node.id))
            .min_by_key(|node| (!node.is_primary, node.monitor_index))
            .map(|node| node.id.clone())
    }

    fn node_belongs_to(&self, node: &DisplayNodeId, device: DeviceId) -> bool {
        self.layout
            .as_ref()
            .and_then(|layout| layout.node(node))
            .is_some_and(|node| node.device_id == device)
    }

    fn remote_display_device(&self) -> Option<DeviceId> {
        if let Some(device) = self.snapshot.current_display_device {
            if !self.is_local_device(device) {
                return Some(device);
            }
        }
        self.snapshot
            .peer
            .as_ref()
            .map(|peer| peer.id)
            .filter(|device| !self.is_local_device(*device))
    }

    fn is_local_device(&self, device: DeviceId) -> bool {
        self.layout.as_ref().is_some_and(|layout| {
            layout
                .nodes
                .iter()
                .any(|node| node.is_local && node.device_id == device)
        })
    }

    fn set_input_owner(
        &mut self,
        owner: InputOwner,
        effects: &mut Vec<SessionEffect>,
        force_release: bool,
    ) {
        let owner = match owner {
            InputOwner::Remote { device_id } if self.is_local_device(device_id) => {
                InputOwner::Local
            }
            other => other,
        };

        if force_release {
            effects.push(SessionEffect::ReleaseInput);
            if let InputOwner::Remote { device_id } = owner {
                effects.push(SessionEffect::GrantInput(device_id));
            }
            self.snapshot.input_owner = owner;
            return;
        }

        if self.snapshot.input_owner == owner {
            return;
        }
        if self.snapshot.input_owner.device_id().is_some() {
            effects.push(SessionEffect::ReleaseInput);
        }
        if let InputOwner::Remote { device_id } = owner {
            effects.push(SessionEffect::GrantInput(device_id));
        }
        self.snapshot.input_owner = owner;
    }
}
