import { invoke, type InvokeArgs } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { listenBackendEvent } from './events';
import { normalizeApiError } from './errors';
import { invokeMock, subscribeMockEvent } from './mock';
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
  DeviceView,
  FileDropCard,
  InputServiceStatus,
  Layout,
  MediaBridgeInfo,
  MetricsSnapshot,
  PermissionKind,
  PermissionReport,
  SessionSnapshot,
  TransferView,
  UpdateStatusView,
} from './types';

export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export function isMockRuntime(): boolean {
  return !isTauriRuntime();
}

export async function invokeBackend<K extends BackendCommand>(
  command: K,
  args?: CommandArgsMap[K],
): Promise<CommandResultMap[K]> {
  if (!isTauriRuntime()) {
    return invokeMock(command, args);
  }
  try {
    const payload = args === undefined ? undefined : (args as InvokeArgs);
    return await invoke<CommandResultMap[K]>(command, payload);
  } catch (error) {
    throw normalizeApiError(error);
  }
}

export async function subscribeBackendEvent<K extends BackendEventName>(
  event: K,
  handler: (payload: BackendEventMap[K]) => void,
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) {
    return Promise.resolve(subscribeMockEvent(event, handler));
  }
  return listenBackendEvent(event, handler);
}

export async function chooseUpdatePackage(title: string): Promise<string | null> {
  if (!isTauriRuntime()) {
    return '/mock/NexusCat-1.1.0-installer.exe';
  }
  try {
    const selected = await open({
      multiple: false,
      directory: false,
      title,
      filters: [
        {
          name: 'NexusCat update',
          extensions: /Mac/.test(navigator.userAgent) ? ['pkg'] : ['exe', 'msi'],
        },
      ],
    });
    if (Array.isArray(selected)) {
      return selected[0] ?? null;
    }
    return selected;
  } catch (error) {
    throw normalizeApiError(error);
  }
}

export async function chooseFiles(title: string): Promise<string[]> {
  if (!isTauriRuntime()) {
    return ['/mock/NexusCat-demo.zip'];
  }
  try {
    const selected = await open({
      multiple: true,
      directory: false,
      title,
    });
    if (Array.isArray(selected)) {
      return selected;
    }
    return selected ? [selected] : [];
  } catch (error) {
    throw normalizeApiError(error);
  }
}

async function runWindowAction(action: 'minimize' | 'maximize'): Promise<boolean> {
  if (!isTauriRuntime()) {
    return false;
  }
  try {
    const appWindow = getCurrentWindow();
    if (action === 'minimize') {
      await appWindow.minimize();
    } else {
      await appWindow.toggleMaximize();
    }
    return true;
  } catch (error) {
    throw normalizeApiError(error);
  }
}

export const api = {
  isMock: isMockRuntime,
  invoke: invokeBackend,
  subscribe: subscribeBackendEvent,

  getBootstrap: (): Promise<Bootstrap> => invokeBackend('get_bootstrap'),
  getState: (): Promise<SessionSnapshot> => invokeBackend('get_state'),
  getDevices: (): Promise<DeviceView[]> => invokeBackend('get_devices'),
  startDiscovery: (): Promise<void> => invokeBackend('start_discovery'),
  stopDiscovery: (): Promise<void> => invokeBackend('stop_discovery'),
  probeDevices: (): Promise<void> => invokeBackend('probe_devices'),
  pairDevice: (deviceId: DeviceId): Promise<void> =>
    invokeBackend('pair_device', { device_id: deviceId }),
  confirmPairing: (accept: boolean): Promise<void> =>
    invokeBackend('confirm_pairing', { accept }),
  cancelPairing: (): Promise<void> => invokeBackend('cancel_pairing'),
  removeTrustedDevice: (deviceId: DeviceId): Promise<void> =>
    invokeBackend('remove_trusted_device', { device_id: deviceId }),
  connectDevice: (deviceId: DeviceId): Promise<void> =>
    invokeBackend('connect_device', { device_id: deviceId }),
  disconnect: (): Promise<void> => invokeBackend('disconnect'),
  setMode: (mode: 'kvm' | 'aggregation'): Promise<void> =>
    invokeBackend('set_mode', { mode }),
  toggleMode: (): Promise<void> => invokeBackend('toggle_mode'),
  summonStage: (): Promise<void> => invokeBackend('summon_stage'),
  hideStage: (): Promise<void> => invokeBackend('hide_stage'),
  getLayout: (): Promise<Layout> => invokeBackend('get_layout'),
  setLayout: (layout: Layout): Promise<Layout> => invokeBackend('set_layout', { layout }),
  getSettings: (): Promise<AppSettings> => invokeBackend('get_settings'),
  updateSettings: (settings: AppSettings): Promise<AppSettings> =>
    invokeBackend('update_settings', { settings }),
  syncSharedConfig: (): Promise<void> => invokeBackend('sync_shared_config'),
  sendFiles: (paths: string[], nodeId?: string): Promise<void> =>
    invokeBackend(
      'send_files',
      nodeId ? { paths, node_id: nodeId } : { paths },
    ),
  showFileDropOsd: (fileCount: number): Promise<void> =>
    invokeBackend('show_file_drop_osd', { file_count: fileCount }),
  hideFileDropOsd: (): Promise<void> => invokeBackend('hide_file_drop_osd'),
  setFileDropHover: (nodeId: string | null): Promise<void> =>
    invokeBackend('set_file_drop_hover', { node_id: nodeId }),
  setFileDropCards: (
    scale: number,
    cards: FileDropCard[],
  ): Promise<void> => invokeBackend('set_file_drop_cards', { scale, cards }),
  getInputServiceStatus: (): Promise<InputServiceStatus> =>
    invokeBackend('get_input_service_status'),
  installInputService: (): Promise<void> =>
    invokeBackend('install_input_service'),
  uninstallInputService: (): Promise<void> =>
    invokeBackend('uninstall_input_service'),
  getTransfers: (): Promise<TransferView[]> => invokeBackend('get_transfers'),
  cancelTransfer: (transferId: number): Promise<void> =>
    invokeBackend('cancel_transfer', { id: transferId }),
  clearFinishedTransfers: (): Promise<void> =>
    invokeBackend('clear_finished_transfers'),
  openDownloadsFolder: (): Promise<void> =>
    invokeBackend('open_downloads_folder'),
  getNotifications: (): Promise<AppNotification[]> => invokeBackend('get_notifications'),
  markNotificationRead: (id: number): Promise<void> =>
    invokeBackend('mark_notification_read', { id }),
  markAllNotificationsRead: (): Promise<void> =>
    invokeBackend('mark_all_notifications_read'),
  clearNotifications: (): Promise<void> => invokeBackend('clear_notifications'),
  getMetrics: (): Promise<MetricsSnapshot> => invokeBackend('get_metrics'),
  getPermissions: (): Promise<PermissionReport> => invokeBackend('get_permissions'),
  requestPermission: (kind: PermissionKind): Promise<boolean> =>
    invokeBackend('request_permission', { kind }),
  openPermissionSettings: (kind: PermissionKind): Promise<void> =>
    invokeBackend('open_permission_settings', { kind }),
  pushUpdate: (
    path: string,
    force = false,
    kind: 'installer' | 'portable' | null = null,
  ): Promise<void> => invokeBackend('push_update', { path, force, kind }),
  getUpdateStatus: (): Promise<UpdateStatusView> => invokeBackend('get_update_status'),
  applyUpdate: (version: string): Promise<void> =>
    invokeBackend('apply_update', { version }),
  rollbackUpdate: (version: string): Promise<void> =>
    invokeBackend('rollback_update', { version }),
  showControlPanel: (): Promise<void> => invokeBackend('show_control_panel'),
  retryConnection: (): Promise<void> => invokeBackend('retry_connection'),
  reportSurfaceReady: (): Promise<void> => invokeBackend('report_surface_ready'),
  hideToBackground: (): Promise<void> => invokeBackend('hide_to_background'),
  setShortcutCapture: (active: boolean): Promise<void> =>
    invokeBackend('set_shortcut_capture', { active }),
  setControlPanelSize: (
    expanded: boolean,
    scaleFactor: number,
    availableWidth: number,
    availableHeight: number,
  ): Promise<void> =>
    invokeBackend('set_control_panel_size', {
      expanded,
      scale_factor: scaleFactor,
      available_width: availableWidth,
      available_height: availableHeight,
    }),
  quitApp: (): Promise<void> => invokeBackend('quit_app'),
  reportPresentStats: (stats: {
    decode_ms: number;
    present_ms: number;
    frames: number;
    presented_frames: number;
    dropped_frames: number;
    timestamp_us: number;
  }): Promise<void> => invokeBackend('report_present_stats', stats),
  requestKeyframe: (streamId: number): Promise<void> =>
    invokeBackend('request_keyframe', { stream_id: streamId }),
  reportWebCodecsProbe: (result: {
    supported: boolean;
    h264: boolean;
    reason: string | null;
  }): Promise<void> => invokeBackend('webcodecs_probe_result', result),
  getMediaBridge: (): Promise<MediaBridgeInfo> => invokeBackend('get_media_bridge'),
  setAutostart: (enabled: boolean): Promise<void> =>
    invokeBackend('set_autostart', { enabled }),

  chooseUpdatePackage,
  chooseFiles,
  minimizeWindow: (): Promise<boolean> => runWindowAction('minimize'),
  toggleMaximizeWindow: (): Promise<boolean> => runWindowAction('maximize'),
};

export type ApiClient = typeof api;
