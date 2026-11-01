import type {
  AppNotification,
  AppSettings,
  Bootstrap,
  DeviceView,
  GroupStatus,
  Layout,
  MediaBridgeInfo,
  MetricsSnapshot,
  NotificationSeverity,
  PairingEvent,
  PermissionReport,
  SessionSnapshot,
  SharedConfigStatus,
  TransferView,
  UpdateStatusView,
} from '../api/types';

export type AppView = 'workbench' | 'settings';

export type LoadStatus = 'loading' | 'ready' | 'error';
export type StageCommandState = 'show' | null;
export type AppSurface =
  | 'loading'
  | 'error'
  | 'onboarding'
  | 'stage'
  | 'background'
  | 'control';

export interface ToastMessage {
  id: number;
  severity: NotificationSeverity;
  title: string;
  body?: string;
}

export interface AppState {
  status: LoadStatus;
  error: string | null;
  bootstrap: Bootstrap | null;
  settings: AppSettings;
  session: SessionSnapshot | null;
  devices: DeviceView[];
  layout: Layout;
  notifications: AppNotification[];
  permissions: PermissionReport;
  metrics: MetricsSnapshot;
  updateStatus: UpdateStatusView | null;
  sharedConfig: SharedConfigStatus;
  groupStatus: GroupStatus;
  transfers: TransferView[];
  pairing: PairingEvent | null;
  view: AppView;
  toasts: ToastMessage[];
  onboardingChecked: boolean;
  onboardingDone: boolean;
  stageCommand: StageCommandState;
  mediaBridge: MediaBridgeInfo | null;
  updateRequested: number;
}

export type AppAction =
  | { type: 'bootstrap-loading' }
  | { type: 'bootstrap-loaded'; bootstrap: Bootstrap; onboardingDone: boolean }
  | { type: 'bootstrap-failed'; error: string }
  | { type: 'navigate'; view: AppView }
  | { type: 'session-changed'; session: SessionSnapshot }
  | { type: 'devices-changed'; devices: DeviceView[] }
  | { type: 'layout-changed'; layout: Layout }
  | { type: 'notifications-replaced'; notifications: AppNotification[] }
  | { type: 'notification-received'; notification: AppNotification }
  | { type: 'notification-marked-read'; id: number }
  | { type: 'all-notifications-read' }
  | { type: 'notifications-cleared' }
  | { type: 'pairing-event'; event: PairingEvent }
  | { type: 'pairing-closed' }
  | { type: 'metrics'; metrics: MetricsSnapshot }
  | { type: 'transfer-progress'; transfer: TransferView }
  | { type: 'transfers-replaced'; transfers: TransferView[] }
  | { type: 'update-status'; status: UpdateStatusView }
  | { type: 'permissions-changed'; permissions: PermissionReport }
  | { type: 'settings-updated'; settings: AppSettings }
  | { type: 'shared-sync-status'; status: SharedConfigStatus }
  | { type: 'group-status'; status: GroupStatus }
  | { type: 'toast-added'; toast: ToastMessage }
  | { type: 'toast-dismissed'; id: number }
  | { type: 'stage-command'; action: 'show' | 'hide' }
  | { type: 'onboarding-opened' }
  | { type: 'onboarding-completed' }
  | { type: 'open-update' }
  | { type: 'media-restart'; info: MediaBridgeInfo };

export const EMPTY_SETTINGS: AppSettings = {
  device_name: '',
  language: 'zh',
  theme: 'system',
  mode: 'kvm',
  edge_enabled: true,
  edge_inset: true,
  stage_on_primary: false,
  shortcuts: {
    toggle_mode: 'Ctrl+Alt+M',
    toggle_stage: 'Ctrl+Alt+F',
    open_calibration: 'Ctrl+Alt+C',
    pause_remote: 'Ctrl+Alt+Space',
    toggle_edge: 'Ctrl+Alt+E',
  },
  autostart: false,
  audio_enabled: true,
  clipboard_enabled: true,
  file_transfer_enabled: true,
  update_auto_accept: false,
  target: { width: 1920, height: 1080, fps: 60, bitrate_kbps: 16000 },
  video_targets: {},
  download_dir: null,
  control_port: 0,
  discovery_port: 0,
  shared_revision: 0,
  shared_updated_at_ms: 0,
  shared_source: null,
};

export const EMPTY_SHARED_CONFIG_STATUS: SharedConfigStatus = {
  state: 'local',
  revision: 0,
  synced_devices: 0,
  online_devices: 0,
  total_devices: 0,
};

export const EMPTY_METRICS: MetricsSnapshot = {
  fps: 0,
  capture_fps: 0,
  received_fps: 0,
  presented_fps: 0,
  bitrate_kbps: 0,
  loss_percent: 0,
  rtt_ms: 0,
  capture_ms: 0,
  encode_ms: 0,
  network_ms: 0,
  decode_ms: 0,
  present_ms: 0,
  first_frame_ms: null,
  glass_to_glass_p50_ms: null,
  glass_to_glass_p95_ms: null,
  nack_count: 0,
  frames_sent: 0,
  frames_received: 0,
  frames_presented: 0,
  dropped_frames: 0,
  updated_at_ms: 0,
};

const EMPTY_GROUP_STATUS: GroupStatus = {
  active_host: null,
  pending_target: null,
  takeover_target: null,
};

export function createInitialState(): AppState {
  return {
    status: 'loading',
    error: null,
    bootstrap: null,
    settings: EMPTY_SETTINGS,
    session: null,
    devices: [],
    layout: {
      nodes: [],
      revision: 0,
      active_node_ids: [],
      orientation_overrides: {},
    },
    notifications: [],
    permissions: { statuses: [] },
    metrics: EMPTY_METRICS,
    updateStatus: null,
    sharedConfig: EMPTY_SHARED_CONFIG_STATUS,
    groupStatus: EMPTY_GROUP_STATUS,
    transfers: [],
    pairing: null,
    view: 'workbench',
    toasts: [],
    onboardingChecked: false,
    onboardingDone: false,
    stageCommand: null,
    mediaBridge: null,
    updateRequested: 0,
  };
}

function sortNotifications(notifications: AppNotification[]): AppNotification[] {
  return [...notifications].sort((left, right) => right.timestamp_ms - left.timestamp_ms);
}

function showsStage(session: SessionSnapshot | null): boolean {
  return Boolean(session && (session.stage_visible || session.ui_state === 'stage'));
}

function reconcileStageCommand(
  current: StageCommandState,
  _previousSession: SessionSnapshot | null,
  _nextSession: SessionSnapshot,
): StageCommandState {
  return current === 'show' ? null : current;
}

export function selectSurface(state: AppState): AppSurface {
  if (state.status === 'loading' || !state.onboardingChecked) {
    return 'loading';
  }
  if (state.status === 'error') {
    return 'error';
  }
  if (!state.onboardingDone) {
    return 'onboarding';
  }
  if (state.stageCommand === 'show' || showsStage(state.session)) {
    return 'stage';
  }
  if (state.session?.ui_state === 'background') {
    return 'background';
  }
  return 'control';
}

export function unreadNotificationCount(state: AppState): number {
  return state.notifications.reduce(
    (count, notification) => (notification.read ? count : count + 1),
    0,
  );
}

export function appReducer(state: AppState, action: AppAction): AppState {
  switch (action.type) {
    case 'bootstrap-loading':
      return {
        ...state,
        status: 'loading',
        error: null,
      };
    case 'bootstrap-loaded':
      return {
        ...state,
        status: 'ready',
        error: null,
        bootstrap: action.bootstrap,
        settings: action.bootstrap.settings,
        session: action.bootstrap.session,
        devices: action.bootstrap.devices,
        layout: action.bootstrap.layout,
        notifications: sortNotifications(action.bootstrap.notifications),
        transfers: action.bootstrap.transfers,
        permissions: action.bootstrap.permissions,
        metrics: action.bootstrap.metrics,
        sharedConfig: action.bootstrap.shared_config,
        groupStatus: action.bootstrap.group_status,
        mediaBridge: action.bootstrap.media_bridge,
        onboardingChecked: true,
        onboardingDone: action.onboardingDone,
        stageCommand: null,
        view: 'workbench',
      };
    case 'bootstrap-failed':
      return {
        ...state,
        status: 'error',
        error: action.error,
        onboardingChecked: true,
      };
    case 'navigate':
      return {
        ...state,
        view: action.view,
      };
    case 'session-changed': {
      const stageCommand = reconcileStageCommand(
        state.stageCommand,
        state.session,
        action.session,
      );
      const nextSettings: AppSettings = {
        ...state.settings,
        mode: action.session.mode,
      };
      return {
        ...state,
        session: action.session,
        settings: nextSettings,
        stageCommand,
      };
    }
    case 'devices-changed':
      return {
        ...state,
        devices: action.devices,
      };
    case 'layout-changed':
      return {
        ...state,
        layout: action.layout,
      };
    case 'notifications-replaced':
      return {
        ...state,
        notifications: sortNotifications(action.notifications),
      };
    case 'notification-received': {
      const exists = state.notifications.some(
        (notification) => notification.id === action.notification.id,
      );
      const notifications = exists
        ? state.notifications.map((notification) =>
            notification.id === action.notification.id
              ? action.notification
              : notification,
          )
        : [action.notification, ...state.notifications];
      return {
        ...state,
        notifications: sortNotifications(notifications),
      };
    }
    case 'notification-marked-read':
      return {
        ...state,
        notifications: state.notifications.map((notification) =>
          notification.id === action.id ? { ...notification, read: true } : notification,
        ),
      };
    case 'all-notifications-read':
      return {
        ...state,
        notifications: state.notifications.map((notification) => ({
          ...notification,
          read: true,
        })),
      };
    case 'notifications-cleared':
      return {
        ...state,
        notifications: [],
      };
    case 'pairing-event':
      return {
        ...state,
        pairing: action.event,
      };
    case 'pairing-closed':
      return {
        ...state,
        pairing: null,
      };
    case 'metrics':
      return {
        ...state,
        metrics: action.metrics,
      };
    case 'transfer-progress': {
      const exists = state.transfers.some((transfer) => transfer.id === action.transfer.id);
      return {
        ...state,
        transfers: exists
          ? state.transfers.map((transfer) =>
              transfer.id === action.transfer.id ? action.transfer : transfer,
            )
          : [action.transfer, ...state.transfers],
      };
    }
    case 'transfers-replaced':
      return {
        ...state,
        transfers: action.transfers,
      };
    case 'update-status':
      return {
        ...state,
        updateStatus: action.status,
      };
    case 'permissions-changed':
      return {
        ...state,
        permissions: action.permissions,
      };
    case 'settings-updated':
      return {
        ...state,
        settings: action.settings,
      };
    case 'shared-sync-status':
      return {
        ...state,
        sharedConfig: action.status,
      };
    case 'group-status':
      return {
        ...state,
        groupStatus: action.status,
      };
    case 'toast-added':
      return {
        ...state,
        toasts: [...state.toasts.slice(-4), action.toast],
      };
    case 'toast-dismissed':
      return {
        ...state,
        toasts: state.toasts.filter((toast) => toast.id !== action.id),
      };
    case 'stage-command':
      if (action.action === 'hide') {
        return state;
      }
      return {
        ...state,
        stageCommand: action.action,
      };
    case 'onboarding-opened':
      return {
        ...state,
        onboardingChecked: true,
        onboardingDone: false,
      };
    case 'onboarding-completed':
      return {
        ...state,
        onboardingChecked: true,
        onboardingDone: true,
      };
    case 'open-update':
      return {
        ...state,
        view: 'settings',
        updateRequested: state.updateRequested + 1,
      };
    case 'media-restart':
      return {
        ...state,
        mediaBridge: action.info,
      };
    default:
      return state;
  }
}
