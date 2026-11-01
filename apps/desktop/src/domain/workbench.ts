import type {
  DeviceView,
  DisplayNode,
  GroupStatus,
  Layout,
  Platform,
  SessionSnapshot,
} from '../api/types';
import {
  currentControlDeviceId,
  displaysForDevice,
  localDeviceId,
  primaryDisplayForDevice,
} from './group';
import { directionFromLocal, type LayoutDirection } from './layout';

export type DeviceActionKind =
  | 'none'
  | 'pair'
  | 'connect'
  | 'control'
  | 'disconnect'
  | 'takeover';

export interface DeviceRailEntry {
  id: string;
  name: string;
  isLocal: boolean;
  online: boolean;
  trusted: boolean;
  connected: boolean;
  currentControl: boolean;
  isHost: boolean;
  isPending: boolean;
  isTakeoverTarget: boolean;
  isSessionPeer: boolean;
  remoteHostBlocks: boolean;
  platform: Platform;
  displayCount: number;
  displayNames: string[];
  primaryNodeId: string | null;
  direction: LayoutDirection | null;
  address: string | null;
  lastSeenMs: number;
}

function displayLabel(node: DisplayNode, monitorIndexLabel: (index: number) => string): string {
  return node.display_name.trim() || monitorIndexLabel(node.monitor_index);
}

export function buildDeviceRailEntries(
  devices: DeviceView[],
  layout: Layout,
  session: SessionSnapshot | null,
  local: { id: string | null; name: string; platform: Platform },
  monitorIndexLabel: (index: number) => string,
  group: GroupStatus,
): DeviceRailEntry[] {
  const currentControlId = currentControlDeviceId(session);
  const localId = local.id ?? localDeviceId(layout);
  const sessionPeerId = session?.peer?.id ?? null;
  const activeHost = group.active_host;
  const remoteHostBlocks = activeHost !== null && activeHost !== localId;
  const active = layout.nodes.filter((node) =>
    layout.active_node_ids.includes(node.id),
  );
  const entries: DeviceRailEntry[] = [];

  if (localId) {
    const displays = displaysForDevice(layout, localId);
    const primary = primaryDisplayForDevice(layout, localId);
    entries.push({
      id: localId,
      name: local.name,
      isLocal: true,
      online: true,
      trusted: true,
      connected: true,
      currentControl: currentControlId === localId,
      isHost: activeHost === localId,
      isPending: group.pending_target === localId,
      isTakeoverTarget: group.takeover_target === localId,
      isSessionPeer: sessionPeerId === localId,
      remoteHostBlocks: false,
      platform: local.platform,
      displayCount: displays.length,
      displayNames: displays.map((node) => displayLabel(node, monitorIndexLabel)),
      primaryNodeId: primary?.id ?? displays[0]?.id ?? null,
      direction: null,
      address: null,
      lastSeenMs: Date.now(),
    });
  }

  for (const device of devices) {
    const displays = displaysForDevice(layout, device.info.id);
    const primary = primaryDisplayForDevice(layout, device.info.id);
    entries.push({
      id: device.info.id,
      name: device.info.name,
      isLocal: false,
      online: device.online,
      trusted: device.trusted,
      connected: device.connected,
      currentControl: currentControlId === device.info.id,
      isHost: activeHost === device.info.id,
      isPending: group.pending_target === device.info.id,
      isTakeoverTarget: group.takeover_target === device.info.id,
      isSessionPeer: sessionPeerId === device.info.id,
      remoteHostBlocks,
      platform: device.info.platform,
      displayCount: displays.length,
      displayNames: displays.map((node) => displayLabel(node, monitorIndexLabel)),
      primaryNodeId: primary?.id ?? displays[0]?.id ?? null,
      direction: primary
        ? directionFromLocal(active, primary.id)
        : null,
      address: device.address,
      lastSeenMs: device.last_seen_ms,
    });
  }

  return entries.sort(compareEntries);
}

function compareEntries(left: DeviceRailEntry, right: DeviceRailEntry): number {
  const rank = (entry: DeviceRailEntry): number => {
    if (entry.isHost) {
      return 0;
    }
    if (entry.currentControl || entry.isSessionPeer) {
      return 1;
    }
    if (entry.isLocal) {
      return 2;
    }
    if (!entry.trusted) {
      return 5;
    }
    return entry.online ? 3 : 4;
  };
  const difference = rank(left) - rank(right);
  if (difference !== 0) {
    return difference;
  }
  return left.name.localeCompare(right.name);
}

export function deviceActionKind(entry: DeviceRailEntry): DeviceActionKind {
  if (entry.isLocal) {
    return 'none';
  }
  if (!entry.trusted) {
    return 'pair';
  }
  if (!entry.online) {
    return 'none';
  }
  if (entry.remoteHostBlocks) {
    return 'takeover';
  }
  if (entry.currentControl || entry.isSessionPeer) {
    return 'disconnect';
  }
  return entry.connected ? 'control' : 'connect';
}
