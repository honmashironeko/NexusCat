import type {
  AppNotification,
  AppTheme,
  ConnectionMode,
  NotificationCategory,
  NotificationAction,
  NotificationSeverity,
  PermissionKind,
  Platform,
  SessionState,
  UpdateStatusView,
} from '../api/types';
import type { TranslationKey } from '../i18n';
import type { LayoutDirection } from './layout';

export function sessionStateKey(state: SessionState): TranslationKey {
  switch (state) {
    case 'idle':
      return 'session.state.idle';
    case 'discovering':
      return 'session.state.discovering';
    case 'pairing':
      return 'session.state.pairing';
    case 'connected':
      return 'session.state.connected';
    case 'kvm':
      return 'session.state.kvm';
    case 'aggregating':
      return 'session.state.aggregating';
    case 'reconnecting':
      return 'session.state.reconnecting';
    case 'error':
      return 'session.state.error';
    case 'closed':
      return 'session.state.closed';
    default:
      return 'session.state.idle';
  }
}

export function modeLabelKey(mode: ConnectionMode): TranslationKey {
  return mode === 'kvm' ? 'session.mode.kvm' : 'session.mode.aggregation';
}

export function directionLabelKey(direction: LayoutDirection): TranslationKey {
  switch (direction) {
    case 'left':
      return 'session.direction.left';
    case 'right':
      return 'session.direction.right';
    case 'up':
      return 'session.direction.up';
    case 'down':
      return 'session.direction.down';
    default:
      return 'session.direction.left';
  }
}

export function themeLabelKey(theme: AppTheme): TranslationKey {
  switch (theme) {
    case 'system':
      return 'theme.system';
    case 'light':
      return 'theme.light';
    case 'dark':
      return 'theme.dark';
    default:
      return 'theme.system';
  }
}

export function platformLabelKey(platform: Platform): TranslationKey {
  return platform === 'windows' ? 'devices.platform.windows' : 'devices.platform.mac_os';
}

export function permissionKindKey(kind: PermissionKind): TranslationKey {
  switch (kind) {
    case 'screen_recording':
      return 'permissions.kind.screen_recording';
    case 'accessibility':
      return 'permissions.kind.accessibility';
    case 'input_monitoring':
      return 'permissions.kind.input_monitoring';
    default:
      return 'common.unknown';
  }
}

export function notificationCategoryKey(category: NotificationCategory): TranslationKey {
  switch (category) {
    case 'connection':
      return 'notifications.category.connection';
    case 'pairing':
      return 'notifications.category.pairing';
    case 'permission':
      return 'notifications.category.permission';
    case 'update':
      return 'notifications.category.update';
    case 'file_transfer':
      return 'notifications.category.file_transfer';
    case 'media':
      return 'notifications.category.media';
    case 'system':
      return 'notifications.category.system';
    default:
      return 'notifications.category.system';
  }
}

export function notificationActionKey(kind: NotificationAction['kind']): TranslationKey {
  switch (kind) {
    case 'open_control_panel':
      return 'notifications.action.open_control_panel';
    case 'open_settings':
      return 'notifications.action.open_settings';
    case 'show_stage':
      return 'notifications.action.show_stage';
    case 'retry_connection':
      return 'notifications.action.retry_connection';
    case 'open_update':
      return 'notifications.action.open_update';
    case 'dismiss':
      return 'notifications.action.dismiss';
    default:
      return 'notifications.action.dismiss';
  }
}

export function notificationSeverityIcon(
  severity: NotificationSeverity,
): 'info' | 'check' | 'warning' | 'error' {
  switch (severity) {
    case 'info':
      return 'info';
    case 'success':
      return 'check';
    case 'warning':
      return 'warning';
    case 'error':
      return 'error';
    default:
      return 'info';
  }
}

export function updateStateKey(state: UpdateStatusView['state']): TranslationKey {
  switch (state) {
    case 'idle':
      return 'updates.state.idle';
    case 'available':
      return 'updates.state.available';
    case 'receiving':
      return 'updates.state.receiving';
    case 'staged':
      return 'updates.state.staged';
    case 'applying':
      return 'updates.state.applying';
    case 'done':
      return 'updates.state.done';
    case 'failed':
      return 'updates.state.failed';
    default:
      return 'updates.state.idle';
  }
}

export function updateKindKey(kind: UpdateStatusView['kind']): TranslationKey | null {
  if (kind === 'portable') {
    return 'updates.kind.portable';
  }
  if (kind === 'installer') {
    return 'updates.kind.installer';
  }
  return null;
}

export function isNotificationUnread(notification: AppNotification): boolean {
  return !notification.read;
}
