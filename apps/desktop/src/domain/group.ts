import type { DeviceView, DisplayNode, Layout, SessionSnapshot } from '../api/types';
import { primaryNodeForDevice } from './layout';

export type DeviceGroupSection = 'current' | 'online' | 'offline' | 'pending';

export function trustedGroupCount(devices: DeviceView[]): number {
  return 1 + devices.filter((device) => device.trusted).length;
}

export function activeNodes(layout: Layout): DisplayNode[] {
  const active = new Set(layout.active_node_ids);
  return layout.nodes.filter((node) => active.has(node.id));
}

export function displaysForDevice(layout: Layout, deviceId: string | null): DisplayNode[] {
  if (!deviceId) {
    return [];
  }
  return activeNodes(layout)
    .filter((node) => node.device_id === deviceId)
    .sort((left, right) => {
      if (left.is_primary !== right.is_primary) {
        return left.is_primary ? -1 : 1;
      }
      return left.monitor_index - right.monitor_index;
    });
}

export function primaryDisplayForDevice(
  layout: Layout,
  deviceId: string | null,
): DisplayNode | null {
  return primaryNodeForDevice(activeNodes(layout), deviceId);
}

export function localDeviceId(layout: Layout): string | null {
  return layout.nodes.find((node) => node.is_local)?.device_id ?? null;
}

export function currentControlDeviceId(session: SessionSnapshot | null): string | null {
  if (!session || session.input_owner === 'local') {
    return null;
  }
  return session.input_owner.remote.device_id;
}

export function currentNodeForSession(
  layout: Layout,
  session: SessionSnapshot | null,
): DisplayNode | null {
  if (!session?.current_display_node) {
    return null;
  }
  return (
    activeNodes(layout).find((node) => node.id === session.current_display_node) ??
    null
  );
}

export function displayForControl(
  layout: Layout,
  session: SessionSnapshot | null,
  deviceId: string | null,
): DisplayNode | null {
  const current = currentNodeForSession(layout, session);
  if (current && (!deviceId || current.device_id === deviceId)) {
    return current;
  }
  return primaryDisplayForDevice(layout, deviceId);
}

export function groupSectionFor(
  device: DeviceView,
  currentControlId: string | null,
): DeviceGroupSection {
  if (device.info.id === currentControlId) {
    return 'current';
  }
  if (!device.trusted) {
    return 'pending';
  }
  return device.online ? 'online' : 'offline';
}

export function sortGroupDevices(
  devices: DeviceView[],
  currentControlId: string | null,
): DeviceView[] {
  const rank: Record<DeviceGroupSection, number> = {
    current: 0,
    online: 1,
    offline: 2,
    pending: 3,
  };
  return [...devices].sort((left, right) => {
    const leftSection = groupSectionFor(left, currentControlId);
    const rightSection = groupSectionFor(right, currentControlId);
    if (leftSection !== rightSection) {
      return rank[leftSection] - rank[rightSection];
    }
    return left.info.name.localeCompare(right.info.name);
  });
}

export function groupSectionDevices(
  devices: DeviceView[],
  currentControlId: string | null,
): Record<DeviceGroupSection, DeviceView[]> {
  const result: Record<DeviceGroupSection, DeviceView[]> = {
    current: [],
    online: [],
    offline: [],
    pending: [],
  };
  for (const device of sortGroupDevices(devices, currentControlId)) {
    result[groupSectionFor(device, currentControlId)].push(device);
  }
  return result;
}
