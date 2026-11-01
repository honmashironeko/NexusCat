
import { ApiError } from './errors';
import type {
  AppNotification,
  AppSettings,
  BackendCommand,
  BackendEventMap,
  BackendEventName,
  Bootstrap,
  CommandArgsMap,
  CommandResultMap,
  DeviceId,
  DeviceInfo,
  DeviceView,
  DisplayNode,
  GroupStatus,
  Layout,
  MediaBridgeInfo,
  MetricsSnapshot,
  NotificationAction,
  NotificationCategory,
  NotificationSeverity,
  PairingEvent,
  PermissionReport,
  PermissionStatus,
  SessionSnapshot,
  SharedConfigStatus,
  TransferView,
  UpdateStatusView,
} from './types';

const LOCAL_ID = 'a1b2c3d4e5f60718293a4b5c6d7e8f90';
const MAC_ID = '0f1e2d3c4b5a69788796a5b4c3d2e1f0';
const OFFICE_ID = '11223344556677889900aabbccddeeff';

const wait = (ms: number): Promise<void> =>
  new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });

function clone<T>(value: T): T {
  return structuredClone(value);
}

function deviceInfo(input: {
  id: DeviceId;
  name: string;
  platform: DeviceInfo['platform'];
  controlPort: number;
  mediaPort: number;
  maxWidth: number;
  maxHeight: number;
  maxFps: number;
  hardwareEncode?: boolean;
}): DeviceInfo {
  return {
    id: input.id,
    name: input.name,
    platform: input.platform,
    capabilities: {
      screen_capture: true,
      input_inject: true,
      audio_capture: true,
      clipboard: true,
      file_transfer: true,
      hardware_encode: input.hardwareEncode ?? true,
      max_width: input.maxWidth,
      max_height: input.maxHeight,
      max_fps: input.maxFps,
    },
    control_port: input.controlPort,
    media_port: input.mediaPort,
  };
}

const localDevice = deviceInfo({
  id: LOCAL_ID,
  name: 'NexusCat-PC',
  platform: 'windows',
  controlPort: 47820,
  mediaPort: 47821,
  maxWidth: 2560,
  maxHeight: 1440,
  maxFps: 120,
});

const macDevice = deviceInfo({
  id: MAC_ID,
  name: 'MacBook',
  platform: 'mac_os',
  controlPort: 51220,
  mediaPort: 51221,
  maxWidth: 3024,
  maxHeight: 1964,
  maxFps: 120,
});

const officeDevice = deviceInfo({
  id: OFFICE_ID,
  name: 'Office PC',
  platform: 'windows',
  controlPort: 53220,
  mediaPort: 53221,
  maxWidth: 1920,
  maxHeight: 1080,
  maxFps: 60,
});

const initialSettings: AppSettings = {
  device_name: 'NexusCat-PC',
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
  autostart: true,
  audio_enabled: true,
  clipboard_enabled: true,
  file_transfer_enabled: true,
  update_auto_accept: false,
  target: { width: 1920, height: 1080, fps: 60, bitrate_kbps: 16000 },
  video_targets: {},
  download_dir: null,
  control_port: 47820,
  discovery_port: 47822,
  shared_revision: 0,
  shared_updated_at_ms: 0,
  shared_source: null,
};

const initialLayout: Layout = {
  revision: 3,
  active_node_ids: [`${LOCAL_ID}:0`, `${MAC_ID}:0`],
  orientation_overrides: {},
  nodes: [
    {
      id: `${LOCAL_ID}:0`,
      device_id: LOCAL_ID,
      device_name: localDevice.name,
      display_name: '外接显示器',
      monitor_index: 0,
      resolution: { width: 2560, height: 1440 },
      dpi: 100,
      physical: { width_mm: 597, height_mm: 336 },
      origin: { x: 0, y: 0 },
      is_local: true,
      is_primary: true,
    },
    {
      id: `${MAC_ID}:0`,
      device_id: MAC_ID,
      device_name: macDevice.name,
      display_name: '内置显示器',
      monitor_index: 0,
      resolution: { width: 3024, height: 1964 },
      dpi: 200,
      physical: { width_mm: 345, height_mm: 224 },
      origin: { x: 2584, y: -262 },
      is_local: false,
      is_primary: true,
    },
  ],
};

const initialPermissions: PermissionReport = {
  statuses: [
    { kind: 'screen_recording', granted: true },
    { kind: 'accessibility', granted: true },
    { kind: 'input_monitoring', granted: true },
  ],
};

const now = Date.now();
const initialNotifications: AppNotification[] = [
  {
    id: 3,
    severity: 'info',
    category: 'update',
    title: 'NexusCat 1.1.0',
    body: 'MacBook · 1.1.0',
    action: {
      kind: 'open_update',
      label: 'View',
      payload: null,
    },
    timestamp_ms: now - 1000 * 60 * 4,
    read: false,
  },
  {
    id: 2,
    severity: 'success',
    category: 'connection',
    title: 'MacBook',
    body: '192.0.2.42 · 8 ms',
    action: {
      kind: 'open_control_panel',
      label: 'Open',
      payload: null,
    },
    timestamp_ms: now - 1000 * 60 * 19,
    read: false,
  },
  {
    id: 1,
    severity: 'info',
    category: 'system',
    title: 'NexusCat',
    body: 'v0.1.0',
    action: null,
    timestamp_ms: now - 1000 * 60 * 60 * 3,
    read: true,
  },
];

const initialMetrics: MetricsSnapshot = {
  fps: 60,
  capture_fps: 60,
  received_fps: 0,
  presented_fps: 0,
  bitrate_kbps: 16240,
  loss_percent: 0.02,
  rtt_ms: 11,
  capture_ms: 2.4,
  encode_ms: 3.8,
  network_ms: 2.1,
  decode_ms: 3.1,
  present_ms: 1.2,
  first_frame_ms: 142,
  glass_to_glass_p50_ms: 18,
  glass_to_glass_p95_ms: 27,
  nack_count: 0,
  frames_sent: 54210,
  frames_received: 0,
  frames_presented: 0,
  dropped_frames: 2,
  updated_at_ms: now,
};

const initialUpdateStatus: UpdateStatusView = {
  state: 'available',
  version: '1.1.0',
  kind: 'installer',
  progress_percent: 0,
  message: null,
  rollback_versions: ['0.9.0'],
  peers: [],
};

interface MockStore {
  settings: AppSettings;
  session: SessionSnapshot;
  groupStatus: GroupStatus;
  devices: DeviceView[];
  layout: Layout;
  notifications: AppNotification[];
  permissions: PermissionReport;
  metrics: MetricsSnapshot;
  updateStatus: UpdateStatusView;
  transfers: TransferView[];
}

const initialDevices: DeviceView[] = [
  {
    info: macDevice,
    online: true,
    connected: true,
    trusted: true,
    address: '192.0.2.42:51220',
    last_seen_ms: now,
  },
  {
    info: officeDevice,
    online: true,
    connected: false,
    trusted: false,
    address: '192.0.2.77:53220',
    last_seen_ms: now - 12_000,
  },
];

function mockSharedStatus(): SharedConfigStatus {
  const trusted = store.devices.filter((device) => device.trusted);
  const online = trusted.filter((device) => device.online).length;
  return {
    state: trusted.length > 0 && online > 0 ? 'synced' : 'local',
    revision: store.settings.shared_revision,
    synced_devices: online,
    online_devices: online,
    total_devices: trusted.length,
  };
}

const store: MockStore = {
  settings: initialSettings,
  session: {
    state: 'kvm',
    phase: 'ready',
    mode: 'kvm',
    ui_state: 'control_panel',
    peer: macDevice,
    input_owner: { remote: { device_id: MAC_ID } },
    stage_visible: false,
    connected_since_ms: now - 1000 * 60 * 34,
    last_error: null,
    current_display_device: MAC_ID,
    current_display_node: `${MAC_ID}:0`,
  },
  groupStatus: {
    active_host: LOCAL_ID,
    pending_target: null,
    takeover_target: null,
  },
  devices: initialDevices,
  layout: initialLayout,
  notifications: initialNotifications,
  permissions: initialPermissions,
  metrics: initialMetrics,
  updateStatus: initialUpdateStatus,
  transfers: [],
};

let notificationSequence = 100;
let transferSequence = 0;
let pairingTarget: DeviceId | null = null;
let stageReturnUi: 'control_panel' | 'background' = 'control_panel';
let pendingSas = false;
let updateTimer: ReturnType<typeof setInterval> | null = null;
let metricsTimer: ReturnType<typeof setInterval> | null = null;

function updateTransfer(
  id: number,
  patch: Partial<TransferView>,
): void {
  store.transfers = store.transfers.map((transfer) =>
    transfer.id === id ? { ...transfer, ...patch } : transfer,
  );
  const updated = store.transfers.find((transfer) => transfer.id === id);
  if (updated) {
    emit('transfer-progress', updated);
  }
}

type MockListener = (payload: unknown) => void;
const listeners = new Map<BackendEventName, Set<MockListener>>();

export function subscribeMockEvent<K extends BackendEventName>(
  event: K,
  handler: (payload: BackendEventMap[K]) => void,
): () => void {
  let bucket = listeners.get(event);
  if (!bucket) {
    bucket = new Set<MockListener>();
    listeners.set(event, bucket);
  }
  const listener = handler as MockListener;
  bucket.add(listener);
  return () => {
    bucket?.delete(listener);
  };
}

function emit<K extends BackendEventName>(event: K, payload: BackendEventMap[K]): void {
  const bucket = listeners.get(event);
  if (!bucket) {
    return;
  }
  const serialized = clone(payload);
  bucket.forEach((handler) => {
    handler(serialized);
  });
}

function emitSession(): void {
  emit('session-changed', store.session);
}

function emitNotifications(): void {
  emit('notifications-changed', store.notifications);
}

function emitGroupStatus(): void {
  emit('group-status', store.groupStatus);
}

function activePeer(): DeviceInfo | null {
  return store.session.peer;
}

function buildBootstrap(): Bootstrap {
  const mediaBridge: MediaBridgeInfo = { port: 0, token: 'mock-media-bridge' };
  return {
    version: '0.1.0',
    platform: 'windows',
    data_dir: 'C:\\NexusCat\\data',
    settings: store.settings,
    session: store.session,
    devices: store.devices,
    layout: store.layout,
    notifications: store.notifications,
    transfers: store.transfers,
    permissions: store.permissions,
    metrics: store.metrics,
    media_bridge: mediaBridge,
    webcodecs: { supported: true, h264: true, reason: null },
    video_capabilities: {
      class: 'discrete',
      hardware_encode: true,
      max_width: 3840,
      max_height: 2160,
      max_fps: 144,
    },
    shared_config: mockSharedStatus(),
    group_status: store.groupStatus,
    test_mode: false,
  };
}

function addNotification(input: {
  severity: NotificationSeverity;
  category: NotificationCategory;
  title: string;
  body: string;
  action?: NotificationAction | null;
}): AppNotification {
  const notification: AppNotification = {
    id: ++notificationSequence,
    severity: input.severity,
    category: input.category,
    title: input.title,
    body: input.body,
    action: input.action ?? null,
    timestamp_ms: Date.now(),
    read: false,
  };
  store.notifications = [notification, ...store.notifications];
  emit('notification', notification);
  emit('notifications-changed', store.notifications);
  return notification;
}

function updateMetrics(): void {
  if (store.session.state !== 'connected' && store.session.state !== 'kvm' && store.session.state !== 'aggregating') {
    store.metrics = { ...store.metrics, updated_at_ms: Date.now() };
    return;
  }
  const targetFps = store.settings.target.fps;
  const jitter = (base: number, spread: number): number =>
    Math.max(0, base + (Math.random() * 2 - 1) * spread);
  const aggregationFps = store.session.mode === 'aggregation' ? targetFps : 0;
  store.metrics = {
    ...store.metrics,
    fps: Math.round(jitter(store.session.mode === 'aggregation' ? targetFps : 60, 1.8)),
    capture_fps: Math.round(jitter(store.session.mode === 'aggregation' ? targetFps : 60, 1.2)),
    received_fps: Math.round(jitter(aggregationFps, 1.5)),
    presented_fps: Math.round(jitter(aggregationFps, 1.2)),
    bitrate_kbps: Math.round(
      jitter(
        Math.min(store.settings.target.bitrate_kbps, store.session.mode === 'aggregation' ? 24000 : 4000),
        store.session.mode === 'aggregation' ? 1400 : 300,
      ),
    ),
    loss_percent: Number(jitter(0.02, 0.03).toFixed(2)),
    rtt_ms: Math.round(jitter(11, 3)),
    capture_ms: Number(jitter(2.4, 0.4).toFixed(1)),
    encode_ms: Number(jitter(3.8, 0.6).toFixed(1)),
    network_ms: Number(jitter(2.1, 0.5).toFixed(1)),
    decode_ms: Number(jitter(3.1, 0.5).toFixed(1)),
    present_ms: Number(jitter(1.2, 0.3).toFixed(1)),
    dropped_frames: store.metrics.dropped_frames + (Math.random() > 0.94 ? 1 : 0),
    frames_received:
      store.metrics.frames_received + (store.session.mode === 'aggregation' ? targetFps : 0),
    frames_presented:
      store.metrics.frames_presented + (store.session.mode === 'aggregation' ? targetFps : 0),
    updated_at_ms: Date.now(),
  };
  emit('metrics', store.metrics);
}

function startMockClock(): void {
  if (metricsTimer !== null) {
    return;
  }
  metricsTimer = window.setInterval(updateMetrics, 1000);
}

function findDevice(deviceId: DeviceId): DeviceView | undefined {
  return store.devices.find((device) => device.info.id === deviceId);
}

function setRemoteConnected(deviceId: DeviceId | null, connected: boolean): void {
  const active = store.layout.nodes
    .filter(
      (node) =>
        node.is_local ||
        (deviceId !== null && node.device_id === deviceId && connected),
    )
    .map((node) => node.id);
  const current = store.layout.active_node_ids;
  const changed =
    active.length !== current.length ||
    active.some((id) => !current.includes(id));
  if (changed) {
    store.layout = {
      ...store.layout,
      active_node_ids: active,
      revision: store.layout.revision + 1,
    };
    emit('layout-changed', store.layout);
  }
}

function appendOfficeNode(): void {
  if (store.layout.nodes.some((node) => node.device_id === OFFICE_ID)) {
    return;
  }
  const node: DisplayNode = {
    id: `${OFFICE_ID}:0`,
    device_id: OFFICE_ID,
    device_name: officeDevice.name,
    display_name: '办公显示器',
    monitor_index: 0,
    resolution: { width: 1920, height: 1080 },
    dpi: 100,
    physical: { width_mm: 527, height_mm: 296 },
    origin: { x: -1944, y: 180 },
    is_local: false,
    is_primary: true,
  };
  store.layout = {
    ...store.layout,
    revision: store.layout.revision + 1,
    nodes: [...store.layout.nodes, node],
  };
  emit('layout-changed', store.layout);
}

function applyMode(mode: 'kvm' | 'aggregation'): void {
  store.settings = { ...store.settings, mode };
  const peer = activePeer();
  const connected = peer !== null;
  store.session = {
    ...store.session,
    mode,
    state: connected ? (mode === 'aggregation' ? 'aggregating' : 'kvm') : 'idle',
    ui_state: 'control_panel',
    stage_visible: false,
    input_owner: 'local',
    current_display_device: LOCAL_ID,
    current_display_node: null,
  };
  emitSession();
}

function startUpdate(path: string): void {
  if (!path.trim()) {
    throw new ApiError('invalid_path', 'Update package path is empty');
  }
  if (updateTimer !== null) {
    clearInterval(updateTimer);
    updateTimer = null;
  }
  const versionMatch = path.match(/(\d+\.\d+\.\d+)/);
  const version = versionMatch?.[1] ?? '1.1.0';
  const lower = path.toLowerCase();
  const kind: 'portable' | 'installer' =
    lower.endsWith('.zip') || lower.includes('portable') ? 'portable' : 'installer';
  store.updateStatus = {
    state: 'receiving',
    version,
    kind,
    progress_percent: 0,
    message: null,
    rollback_versions: store.updateStatus.rollback_versions.includes('1.0.0')
      ? store.updateStatus.rollback_versions
      : ['1.0.0', ...store.updateStatus.rollback_versions],
    peers: activePeer()
      ? [
          {
            device_id: macDevice.id,
            device_name: macDevice.name,
            state: 'transferring',
            progress_percent: 0,
            message: null,
          },
        ]
      : [],
  };
  emit('update-status', store.updateStatus);
  updateTimer = window.setInterval(() => {
    const next = Math.min(100, store.updateStatus.progress_percent + 8 + Math.round(Math.random() * 7));
    store.updateStatus = {
      ...store.updateStatus,
      state: next >= 100 ? 'staged' : 'receiving',
      progress_percent: next,
      peers: store.updateStatus.peers.map((peer) => ({
        ...peer,
        state: next >= 100 ? 'done' : 'transferring',
        progress_percent: next,
      })),
    };
    emit('update-status', store.updateStatus);
    if (next >= 100) {
      if (updateTimer !== null) {
        clearInterval(updateTimer);
        updateTimer = null;
      }
      window.setTimeout(() => {
        store.updateStatus = {
          ...store.updateStatus,
          state: 'applying',
          progress_percent: 100,
        };
        emit('update-status', store.updateStatus);
      }, 450);
      window.setTimeout(() => {
        store.updateStatus = {
          ...store.updateStatus,
          state: 'done',
          progress_percent: 100,
          message: null,
        };
        emit('update-status', store.updateStatus);
        addNotification({
          severity: 'success',
          category: 'update',
          title: `NexusCat ${store.updateStatus.version ?? ''}`.trim(),
          body: `LAN · ${store.updateStatus.version ?? ''}`.trim(),
          action: { kind: 'open_update', label: 'View', payload: null },
        });
      }, 1100);
    }
  }, 190);
}

function pairingEvent(stage: PairingEvent['stage'], extra?: Partial<PairingEvent>): PairingEvent {
  const target = pairingTarget ? findDevice(pairingTarget) : undefined;
  return {
    stage,
    device_id: extra?.device_id ?? target?.info.id ?? null,
    device_name: extra?.device_name ?? target?.info.name ?? null,
    sas: extra?.sas ?? null,
    message: extra?.message ?? null,
  };
}

function requirePairingTarget(): DeviceView {
  if (!pairingTarget) {
    throw new ApiError('no_pairing', 'No pairing in progress');
  }
  const target = findDevice(pairingTarget);
  if (!target) {
    throw new ApiError('device_not_found', 'Device not found');
  }
  return target;
}

export async function invokeMock<K extends BackendCommand>(
  command: K,
  args?: CommandArgsMap[K],
): Promise<CommandResultMap[K]> {
  const result = await runMockCommand(command, args as unknown);
  return clone(result) as CommandResultMap[K];
}

async function runMockCommand(command: BackendCommand, rawArgs: unknown): Promise<unknown> {
  switch (command) {
    case 'get_bootstrap': {
      await wait(220);
      startMockClock();
      return buildBootstrap();
    }
    case 'get_state':
      return store.session;
    case 'get_devices':
      return store.devices;
    case 'start_discovery': {
      await wait(180);
      return undefined;
    }
    case 'stop_discovery': {
      await wait(120);
      return undefined;
    }
    case 'probe_devices': {
      await wait(200);
      return undefined;
    }
    case 'pair_device': {
      const args = rawArgs as CommandArgsMap['pair_device'];
      const device = findDevice(args.device_id);
      if (!device) {
        throw new ApiError('device_not_found', 'Device not found');
      }
      if (!device.online) {
        throw new ApiError('device_offline', 'Device is offline');
      }
      if (device.trusted) {
        throw new ApiError('already_trusted', 'Device is already trusted');
      }
      pairingTarget = args.device_id;
      await wait(320);
      pendingSas = true;
      emit(
        'pairing-event',
        pairingEvent('sas_required', {
          sas: '482 913',
          message: `${device.info.name} · SAS`,
        }),
      );
      return undefined;
    }
    case 'confirm_pairing': {
      const args = rawArgs as CommandArgsMap['confirm_pairing'];
      requirePairingTarget();
      await wait(220);
      if (!args.accept) {
        emit('pairing-event', pairingEvent('cancelled'));
        pairingTarget = null;
        pendingSas = false;
        return undefined;
      }
      if (!pendingSas) {
        throw new ApiError('no_pairing', 'No pairing in progress');
      }
      const target = requirePairingTarget();
      const device = findDevice(target.info.id);
      if (device) {
        device.trusted = true;
        device.last_seen_ms = Date.now();
        store.devices = store.devices.map((item) =>
          item.info.id === device.info.id ? { ...device } : item,
        );
      }
      appendOfficeNode();
      emit('devices-changed', store.devices);
      emit(
        'pairing-event',
        pairingEvent('success', {
          message: target.info.name,
        }),
      );
      pairingTarget = null;
      pendingSas = false;
      return undefined;
    }
    case 'cancel_pairing': {
      if (pairingTarget) {
        emit('pairing-event', pairingEvent('cancelled'));
      }
      pairingTarget = null;
      pendingSas = false;
      return undefined;
    }
    case 'remove_trusted_device': {
      const args = rawArgs as CommandArgsMap['remove_trusted_device'];
      const device = findDevice(args.device_id);
      if (!device) {
        throw new ApiError('device_not_found', 'Device not found');
      }
      if (!device.trusted) {
        throw new ApiError('not_trusted', 'Device is not trusted');
      }
      await wait(180);
      store.layout = {
        ...store.layout,
        revision: store.layout.revision + 1,
        nodes: store.layout.nodes.filter(
          (node) => node.device_id !== device.info.id,
        ),
        active_node_ids: store.layout.active_node_ids.filter(
          (id) => !id.startsWith(`${device.info.id}:`),
        ),
      };
      store.devices = store.devices.map((item) =>
        item.info.id === device.info.id
          ? { ...item, trusted: false, last_seen_ms: Date.now() }
          : item,
      );
      if (store.session.peer?.id === device.info.id) {
        store.session = {
          ...store.session,
          state: 'idle',
          phase: 'offline',
          peer: null,
          input_owner: 'local',
          stage_visible: false,
          connected_since_ms: null,
          current_display_device: LOCAL_ID,
    current_display_node: null,
          ui_state: 'control_panel',
        };
        emitSession();
      }
      emit('devices-changed', store.devices);
      emit('layout-changed', store.layout);
      return undefined;
    }
    case 'connect_device': {
      const args = rawArgs as CommandArgsMap['connect_device'];
      const device = findDevice(args.device_id);
      if (!device) {
        throw new ApiError('device_not_found', 'Device not found');
      }
      if (!device.online) {
        throw new ApiError('device_offline', 'Device is offline');
      }
      if (!device.trusted) {
        throw new ApiError('not_trusted', 'Device is not trusted');
      }
      store.groupStatus = {
        ...store.groupStatus,
        pending_target: device.info.id,
      };
      emitGroupStatus();
      store.session = {
        ...store.session,
        state: 'reconnecting',
        phase: 'tls_handshake',
        last_error: null,
      };
      emitSession();
      await wait(620);
      setRemoteConnected(device.info.id, true);
      const showStage = store.session.mode === 'aggregation';
      const displayDeviceId = showStage ? device.info.id : LOCAL_ID;
      store.session = {
        ...store.session,
        state: showStage ? 'aggregating' : 'kvm',
        phase: 'ready',
        peer: device.info,
        connected_since_ms: Date.now(),
        ui_state: showStage ? 'stage' : 'control_panel',
        stage_visible: showStage,
        last_error: null,
        input_owner: showStage ? { remote: { device_id: device.info.id } } : 'local',
        current_display_device: displayDeviceId,
        current_display_node: null,
      };
      store.groupStatus = {
        active_host: LOCAL_ID,
        pending_target: null,
        takeover_target: null,
      };
      emitGroupStatus();
      emitSession();
      addNotification({
        severity: 'success',
        category: 'connection',
        title: device.info.name,
        body: 'LAN · 8 ms',
        action: { kind: 'open_control_panel', label: 'Open', payload: null },
      });
      return undefined;
    }
    case 'disconnect': {
      await wait(240);
      setRemoteConnected(null, false);
      store.session = {
        ...store.session,
        state: 'idle',
        phase: 'offline',
        peer: null,
        input_owner: 'local',
        ui_state: 'control_panel',
        stage_visible: false,
        connected_since_ms: null,
        current_display_device: LOCAL_ID,
    current_display_node: null,
      };
      emitSession();
      store.groupStatus = {
        active_host: null,
        pending_target: null,
        takeover_target: null,
      };
      emitGroupStatus();
      addNotification({
        severity: 'info',
        category: 'connection',
        title: 'NexusCat',
        body: 'LAN',
        action: null,
      });
      return undefined;
    }
    case 'set_mode': {
      const args = rawArgs as CommandArgsMap['set_mode'];
      applyMode(args.mode);
      return undefined;
    }
    case 'toggle_mode': {
      applyMode(store.session.mode === 'kvm' ? 'aggregation' : 'kvm');
      return undefined;
    }
    case 'summon_stage': {
      const peer = activePeer();
      if (!peer || store.session.mode !== 'aggregation') {
        throw new ApiError('not_aggregating', 'Aggregation is not active');
      }
      if (store.session.ui_state !== 'stage') {
        stageReturnUi = store.session.ui_state === 'background' ? 'background' : 'control_panel';
      }
      store.session = {
        ...store.session,
        state: 'aggregating',
        ui_state: 'stage',
        stage_visible: true,
        input_owner: { remote: { device_id: peer.id } },
        current_display_device: peer.id,
        current_display_node: null,
      };
      emitSession();
      emit('stage-command', { action: 'show' });
      return undefined;
    }
    case 'hide_stage': {
      store.session = {
        ...store.session,
        ui_state: stageReturnUi,
        stage_visible: false,
        input_owner: 'local',
      };
      emitSession();
      emit('stage-command', { action: 'hide' });
      return undefined;
    }
    case 'get_layout':
      return store.layout;
    case 'set_layout': {
      const args = rawArgs as CommandArgsMap['set_layout'];
      store.layout = {
        nodes: args.layout.nodes,
        revision: store.layout.revision + 1,
        active_node_ids: store.layout.active_node_ids.filter((id) =>
          args.layout.nodes.some((node) => node.id === id),
        ),
        orientation_overrides: args.layout.orientation_overrides ?? {},
      };
      emit('layout-changed', store.layout);
      return store.layout;
    }
    case 'get_settings':
      return store.settings;
    case 'update_settings': {
      const args = rawArgs as CommandArgsMap['update_settings'];
      store.settings = {
        ...args.settings,
        target: { ...args.settings.target },
        video_targets: { ...args.settings.video_targets },
      };
      emit('settings-changed', store.settings);
      emit('shared-sync-status', mockSharedStatus());
      store.session = {
        ...store.session,
      };
      return store.settings;
    }
    case 'sync_shared_config': {
      await wait(180);
      emit('shared-sync-status', mockSharedStatus());
      return undefined;
    }
    case 'show_file_drop_osd':
    case 'hide_file_drop_osd':
    case 'set_file_drop_hover':
    case 'set_file_drop_cards':
      return undefined;
    case 'get_input_service_status':
      return {
        installed: false,
        running: false,
        connected: false,
        version: null,
        needs_update: false,
        message: '系统输入服务仅支持 Windows',
      };
    case 'install_input_service':
    case 'uninstall_input_service':
      return undefined;
    case 'send_files': {
      const paths = (rawArgs as CommandArgsMap['send_files']).paths;
      await wait(120);
      paths.forEach((path) => {
        const id = ++transferSequence;
        const fileName = path.replace(/\\/g, '/').split('/').pop() ?? path;
        const transfer: TransferView = {
          id,
          direction: 'send',
          file_name: fileName,
          bytes_done: 0,
          bytes_total: 2_400_000,
          state: 'offered',
          message: null,
        };
        store.transfers = [transfer, ...store.transfers];
        emit('transfer-progress', transfer);
        window.setTimeout(() => {
          updateTransfer(id, { state: 'transferring', bytes_done: 800_000 });
        }, 250);
        window.setTimeout(() => {
          updateTransfer(id, {
            state: 'transferring',
            bytes_done: 1_800_000,
          });
        }, 700);
        window.setTimeout(() => {
          updateTransfer(id, {
            state: 'done',
            bytes_done: 2_400_000,
          });
        }, 1200);
      });
      addNotification({
        severity: 'info',
        category: 'file_transfer',
        title: 'Files',
        body: `${paths.length}`,
        action: null,
      });
      return undefined;
    }
    case 'get_transfers':
      return store.transfers;
    case 'cancel_transfer': {
      const args = rawArgs as CommandArgsMap['cancel_transfer'];
      updateTransfer(args.id, { state: 'cancelled' });
      return undefined;
    }
    case 'clear_finished_transfers':
      store.transfers = store.transfers.filter(
        (transfer) =>
          transfer.state !== 'done' &&
          transfer.state !== 'failed' &&
          transfer.state !== 'cancelled',
      );
      emit('transfers-changed', store.transfers);
      return undefined;
    case 'open_downloads_folder':
      return undefined;
    case 'get_notifications':
      return store.notifications;
    case 'mark_notification_read': {
      const args = rawArgs as CommandArgsMap['mark_notification_read'];
      store.notifications = store.notifications.map((notification) =>
        notification.id === args.id ? { ...notification, read: true } : notification,
      );
      emitNotifications();
      return undefined;
    }
    case 'mark_all_notifications_read': {
      store.notifications = store.notifications.map((notification) => ({
        ...notification,
        read: true,
      }));
      emitNotifications();
      return undefined;
    }
    case 'clear_notifications': {
      store.notifications = [];
      emitNotifications();
      return undefined;
    }
    case 'get_metrics':
      return store.metrics;
    case 'get_permissions':
      return store.permissions;
    case 'request_permission': {
      const args = rawArgs as CommandArgsMap['request_permission'];
      await wait(380);
      const status: PermissionStatus = { kind: args.kind, granted: true };
      const exists = store.permissions.statuses.some((item) => item.kind === args.kind);
      store.permissions = {
        statuses: exists
          ? store.permissions.statuses.map((item) =>
              item.kind === args.kind ? status : item,
            )
          : [...store.permissions.statuses, status],
      };
      emit('permission-changed', store.permissions);
      return true;
    }
    case 'open_permission_settings':
      return undefined;
    case 'push_update': {
      const args = rawArgs as CommandArgsMap['push_update'];
      await wait(120);
      startUpdate(args.path);
      return undefined;
    }
    case 'get_update_status':
      return store.updateStatus;
    case 'apply_update': {
      const args = rawArgs as CommandArgsMap['apply_update'];
      await wait(300);
      store.updateStatus = {
        ...store.updateStatus,
        state: 'done',
        version: args.version,
        progress_percent: 100,
        message: null,
      };
      emit('update-status', store.updateStatus);
      return undefined;
    }
    case 'rollback_update': {
      const args = rawArgs as CommandArgsMap['rollback_update'];
      await wait(300);
      store.updateStatus = {
        ...store.updateStatus,
        state: 'done',
        version: args.version,
        progress_percent: 100,
        message: null,
      };
      emit('update-status', store.updateStatus);
      addNotification({
        severity: 'success',
        category: 'update',
        title: `NexusCat ${args.version}`,
        body: `LAN · ${args.version}`,
        action: { kind: 'open_update', label: 'View', payload: null },
      });
      return undefined;
    }
    case 'show_control_panel': {
      store.session = {
        ...store.session,
        ui_state: 'control_panel',
        stage_visible: false,
        input_owner: 'local',
      };
      emitSession();
      return undefined;
    }
    case 'hide_to_background': {
      store.session = {
        ...store.session,
        ui_state: 'background',
      };
      emitSession();
      return undefined;
    }
    case 'retry_connection':
      return undefined;
    case 'report_surface_ready':
      return undefined;
    case 'set_shortcut_capture':
      return undefined;
    case 'set_control_panel_size':
      return undefined;
    case 'quit_app':
      return undefined;
    case 'report_present_stats':
      return undefined;
    case 'request_keyframe':
      return undefined;
    case 'webcodecs_probe_result':
      return undefined;
    case 'get_media_bridge':
      return { port: 0, token: 'mock-media-bridge' } satisfies MediaBridgeInfo;
    case 'set_autostart': {
      const args = rawArgs as CommandArgsMap['set_autostart'];
      store.settings = { ...store.settings, autostart: args.enabled };
      return undefined;
    }
    default:
      throw new ApiError('unknown_command', `Unknown command: ${String(command)}`);
  }
}
