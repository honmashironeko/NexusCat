export type Platform = 'windows' | 'mac_os';
export type DeviceId = string;
export type ConnectionMode = 'kvm' | 'aggregation';
export type AppLanguage = 'zh' | 'en';
export type AppTheme = 'system' | 'light' | 'dark';

export type SessionState =
  | 'idle'
  | 'discovering'
  | 'pairing'
  | 'connected'
  | 'kvm'
  | 'aggregating'
  | 'reconnecting'
  | 'error'
  | 'closed';

export type SessionPhase =
  | 'offline'
  | 'discovering'
  | 'tls_handshake'
  | 'pairing'
  | 'authenticating'
  | 'ready'
  | 'reconnecting'
  | 'failed';

export type UiState = 'background' | 'control_panel' | 'stage';

export interface DeviceCapabilities {
  screen_capture: boolean;
  input_inject: boolean;
  audio_capture: boolean;
  clipboard: boolean;
  file_transfer: boolean;
  hardware_encode: boolean;
  max_width: number;
  max_height: number;
  max_fps: number;
}

export interface DeviceInfo {
  id: DeviceId;
  name: string;
  platform: Platform;
  capabilities: DeviceCapabilities;
  control_port: number;
  media_port: number;
}

export interface DeviceView {
  info: DeviceInfo;
  online: boolean;
  connected: boolean;
  trusted: boolean;
  address: string | null;
  last_seen_ms: number;
}

export type InputOwner = 'local' | { remote: { device_id: DeviceId } };

export interface SessionSnapshot {
  state: SessionState;
  phase: SessionPhase;
  mode: ConnectionMode;
  ui_state: UiState;
  peer: DeviceInfo | null;
  input_owner: InputOwner;
  stage_visible: boolean;
  connected_since_ms: number | null;
  last_error: string | null;
  current_display_device: DeviceId | null;
  current_display_node: string | null;
}

export interface Resolution {
  width: number;
  height: number;
}

export interface DisplayNode {
  id: string;
  device_id: DeviceId;
  device_name: string;
  display_name: string;
  monitor_index: number;
  resolution: Resolution;
  dpi: number;
  physical: { width_mm: number; height_mm: number };
  origin: { x: number; y: number };
  is_local: boolean;
  is_primary: boolean;
}

export type ScreenOrientation = 'landscape' | 'portrait';

export interface Layout {
  nodes: DisplayNode[];
  revision: number;
  active_node_ids: string[];
  orientation_overrides: Record<string, ScreenOrientation>;
}

export interface TargetFormat {
  width: number;
  height: number;
  fps: number;
  bitrate_kbps: number;
}

export interface ShortcutSettings {
  toggle_mode: string;
  toggle_stage: string;
  open_calibration: string;
  pause_remote: string;
  toggle_edge: string;
}

export interface AppSettings {
  device_name: string;
  language: AppLanguage;
  theme: AppTheme;
  mode: ConnectionMode;
  edge_enabled: boolean;
  edge_inset: boolean;
  stage_on_primary: boolean;
  shortcuts: ShortcutSettings;
  autostart: boolean;
  audio_enabled: boolean;
  clipboard_enabled: boolean;
  file_transfer_enabled: boolean;
  update_auto_accept: boolean;
  target: TargetFormat;
  video_targets: Record<DeviceId, TargetFormat>;
  download_dir: string | null;
  control_port: number;
  discovery_port: number;
  shared_revision: number;
  shared_updated_at_ms: number;
  shared_source: DeviceId | null;
}

export type PermissionKind =
  | 'screen_recording'
  | 'accessibility'
  | 'input_monitoring';

export interface PermissionStatus {
  kind: PermissionKind;
  granted: boolean;
}

export interface PermissionReport {
  statuses: PermissionStatus[];
}

export interface NotificationAction {
  kind:
    | 'open_control_panel'
    | 'open_settings'
    | 'show_stage'
    | 'retry_connection'
    | 'open_update'
    | 'dismiss';
  label: string;
  payload: string | null;
}

export type NotificationSeverity = 'info' | 'success' | 'warning' | 'error';
export type NotificationCategory =
  | 'connection'
  | 'pairing'
  | 'permission'
  | 'update'
  | 'file_transfer'
  | 'media'
  | 'system';

export interface AppNotification {
  id: number;
  severity: NotificationSeverity;
  category: NotificationCategory;
  title: string;
  body: string;
  action: NotificationAction | null;
  timestamp_ms: number;
  read: boolean;
}

export interface MetricsSnapshot {
  fps: number;
  capture_fps: number;
  received_fps: number;
  presented_fps: number;
  bitrate_kbps: number;
  loss_percent: number;
  rtt_ms: number;
  capture_ms: number;
  encode_ms: number;
  network_ms: number;
  decode_ms: number;
  present_ms: number;
  first_frame_ms: number | null;
  glass_to_glass_p50_ms: number | null;
  glass_to_glass_p95_ms: number | null;
  nack_count: number;
  frames_sent: number;
  frames_received: number;
  frames_presented: number;
  dropped_frames: number;
  updated_at_ms: number;
}

export type TransferState =
  | 'offered'
  | 'transferring'
  | 'verifying'
  | 'done'
  | 'failed'
  | 'cancelled';

export interface TransferView {
  id: number;
  direction: 'send' | 'receive';
  file_name: string;
  bytes_done: number;
  bytes_total: number;
  state: TransferState;
  message: string | null;
}

export type UpdateState =
  | 'idle'
  | 'available'
  | 'receiving'
  | 'staged'
  | 'applying'
  | 'done'
  | 'failed';

export type UpdatePeerState =
  | 'queued'
  | 'connecting'
  | 'transferring'
  | 'done'
  | 'failed';

export interface UpdatePeerView {
  device_id: DeviceId;
  device_name: string;
  state: UpdatePeerState;
  progress_percent: number;
  message: string | null;
}

export interface UpdateStatusView {
  state: UpdateState;
  version: string | null;
  kind: 'portable' | 'installer' | null;
  progress_percent: number;
  message: string | null;
  rollback_versions: string[];
  peers: UpdatePeerView[];
}

export interface PairingEvent {
  stage: 'sas_required' | 'success' | 'failed' | 'cancelled';
  device_id: DeviceId | null;
  device_name: string | null;
  sas: string | null;
  message: string | null;
}

export interface MediaBridgeInfo {
  port: number;
  token: string;
}

export interface WebCodecsSupport {
  supported: boolean;
  h264: boolean;
  reason: string | null;
}

export interface VideoCapabilitiesSupport {
  class: 'software' | 'integrated' | 'discrete';
  hardware_encode: boolean;
  max_width: number;
  max_height: number;
  max_fps: number;
}

export interface SharedConfigStatus {
  state: 'local' | 'syncing' | 'synced';
  revision: number;
  synced_devices: number;
  online_devices: number;
  total_devices: number;
}

export interface GroupStatus {
  active_host: DeviceId | null;
  pending_target: DeviceId | null;
  takeover_target: DeviceId | null;
}

export interface Bootstrap {
  version: string;
  platform: Platform;
  data_dir: string;
  settings: AppSettings;
  session: SessionSnapshot;
  devices: DeviceView[];
  layout: Layout;
  notifications: AppNotification[];
  transfers: TransferView[];
  permissions: PermissionReport;
  metrics: MetricsSnapshot;
  media_bridge: MediaBridgeInfo;
  webcodecs: WebCodecsSupport;
  video_capabilities: VideoCapabilitiesSupport;
  shared_config: SharedConfigStatus;
  group_status: GroupStatus;
  test_mode: boolean;
}

export interface RemoteCursorEvent {
  device_id: string;
  x: number;
  y: number;
}

export interface RemotePauseEvent {
  paused: boolean;
  shortcut: string;
}

export interface StageEdgeHintEvent {
  device: string;
  display: string;
  direction: 'left' | 'right' | 'up' | 'down';
}

export interface EdgeOsdEvent {
  direction: 'left' | 'right' | 'up' | 'down';
  kicker: string;
  title: string;
  display: string;
  hint: string;
}

export interface ControlOsdEvent {
  kicker: string;
  title: string;
  subtitle: string;
  hint: string;
}

export interface EdgeHaloEvent {
  direction: 'left' | 'right' | 'up' | 'down';
  title: string;
  display: string;
  hint: string;
  available: boolean;
}

export interface InputServiceStatus {
  installed: boolean;
  running: boolean;
  connected: boolean;
  version: string | null;
  needs_update: boolean;
  message: string | null;
}

export interface FileDropOsdEvent {
  nodes: DisplayNode[];
  file_count: number | null;
  enabled: boolean;
  language: AppLanguage;
}

export interface FileDropCard {
  node_id: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FileDropHoverEvent {
  node_id: string | null;
  file_count: number | null;
}

export interface BackendEventMap {
  'session-changed': SessionSnapshot;
  'devices-changed': DeviceView[];
  'layout-changed': Layout;
  'settings-changed': AppSettings;
  'shared-sync-status': SharedConfigStatus;
  'group-status': GroupStatus;
  notification: AppNotification;
  'notifications-changed': AppNotification[];
  'pairing-event': PairingEvent;
  metrics: MetricsSnapshot;
  'transfer-progress': TransferView;
  'transfers-changed': TransferView[];
  'update-status': UpdateStatusView;
  'permission-changed': PermissionReport;
  'media-restart': MediaBridgeInfo;
  'stage-command': { action: 'show' | 'hide' };
  'stage-switching': { active: boolean };
  'calibration-open': null;
  'open-workbench': null;
  'open-settings': null;
  'open-update': null;
  'edge-osd': EdgeOsdEvent;
  'edge-halo': EdgeHaloEvent;
  'control-osd': ControlOsdEvent | null;
  'file-drop-osd': FileDropOsdEvent;
  'file-drop-osd-hidden': null;
  'file-drop-hover': FileDropHoverEvent;
  'remote-cursor': RemoteCursorEvent;
  'remote-pause': RemotePauseEvent;
  'stage-edge-hint': StageEdgeHintEvent;
  'tray-mode-toggle': null;
  'request-quit': null;
}

export type BackendEventName = keyof BackendEventMap;

export interface CommandError {
  code: string;
  message: string;
}

export interface CommandArgsMap {
  get_bootstrap: undefined;
  get_state: undefined;
  get_devices: undefined;
  start_discovery: undefined;
  stop_discovery: undefined;
  probe_devices: undefined;
  pair_device: { device_id: DeviceId };
  confirm_pairing: { accept: boolean };
  cancel_pairing: undefined;
  remove_trusted_device: { device_id: DeviceId };
  connect_device: { device_id: DeviceId };
  disconnect: undefined;
  set_mode: { mode: ConnectionMode };
  toggle_mode: undefined;
  summon_stage: undefined;
  hide_stage: undefined;
  get_layout: undefined;
  set_layout: { layout: Layout };
  get_settings: undefined;
  update_settings: { settings: AppSettings };
  sync_shared_config: undefined;
  send_files: { paths: string[]; node_id?: string | null };
  show_file_drop_osd: { file_count: number };
  hide_file_drop_osd: undefined;
  set_file_drop_hover: { node_id: string | null };
  set_file_drop_cards: { scale: number; cards: FileDropCard[] };
  get_input_service_status: undefined;
  install_input_service: undefined;
  uninstall_input_service: undefined;
  get_transfers: undefined;
  cancel_transfer: { id: number };
  clear_finished_transfers: undefined;
  open_downloads_folder: undefined;
  get_notifications: undefined;
  mark_notification_read: { id: number };
  mark_all_notifications_read: undefined;
  clear_notifications: undefined;
  get_metrics: undefined;
  get_permissions: undefined;
  request_permission: { kind: PermissionKind };
  open_permission_settings: { kind: PermissionKind };
  push_update: {
    path: string;
    force?: boolean;
    kind?: 'installer' | 'portable' | null;
  };
  get_update_status: undefined;
  apply_update: { version: string };
  rollback_update: { version: string };
  show_control_panel: undefined;
  retry_connection: undefined;
  report_surface_ready: undefined;
  hide_to_background: undefined;
  set_shortcut_capture: { active: boolean };
  set_control_panel_size: {
    expanded: boolean;
    scale_factor: number;
    available_width: number;
    available_height: number;
  };
  quit_app: undefined;
  report_present_stats: {
    decode_ms: number;
    present_ms: number;
    frames: number;
    presented_frames: number;
    dropped_frames: number;
    timestamp_us: number;
  };
  request_keyframe: { stream_id: number };
  webcodecs_probe_result: {
    supported: boolean;
    h264: boolean;
    reason: string | null;
  };
  get_media_bridge: undefined;
  set_autostart: { enabled: boolean };
}

export interface CommandResultMap {
  get_bootstrap: Bootstrap;
  get_state: SessionSnapshot;
  get_devices: DeviceView[];
  start_discovery: void;
  stop_discovery: void;
  probe_devices: void;
  pair_device: void;
  confirm_pairing: void;
  cancel_pairing: void;
  remove_trusted_device: void;
  connect_device: void;
  disconnect: void;
  set_mode: void;
  toggle_mode: void;
  summon_stage: void;
  hide_stage: void;
  get_layout: Layout;
  set_layout: Layout;
  get_settings: AppSettings;
  update_settings: AppSettings;
  sync_shared_config: void;
  send_files: void;
  show_file_drop_osd: void;
  hide_file_drop_osd: void;
  set_file_drop_hover: void;
  set_file_drop_cards: void;
  get_input_service_status: InputServiceStatus;
  install_input_service: void;
  uninstall_input_service: void;
  get_transfers: TransferView[];
  cancel_transfer: void;
  clear_finished_transfers: void;
  open_downloads_folder: void;
  get_notifications: AppNotification[];
  mark_notification_read: void;
  mark_all_notifications_read: void;
  clear_notifications: void;
  get_metrics: MetricsSnapshot;
  get_permissions: PermissionReport;
  request_permission: boolean;
  open_permission_settings: void;
  push_update: void;
  get_update_status: UpdateStatusView;
  apply_update: void;
  rollback_update: void;
  show_control_panel: void;
  retry_connection: void;
  report_surface_ready: void;
  hide_to_background: void;
  set_shortcut_capture: void;
  set_control_panel_size: void;
  quit_app: void;
  report_present_stats: void;
  request_keyframe: void;
  webcodecs_probe_result: void;
  get_media_bridge: MediaBridgeInfo;
  set_autostart: void;
}

export type BackendCommand = keyof CommandArgsMap;
