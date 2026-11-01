import type { DisplayNode } from '../api/types';

export const DEVICE_COLOR_COUNT = 6;

export interface DeviceGroupView {
  deviceId: string;
  deviceName: string;
  isLocal: boolean;
  nodes: DisplayNode[];
  colorIndex: number;
}

export function buildDeviceGroups(nodes: DisplayNode[]): DeviceGroupView[] {
  const byDevice = new Map<string, DisplayNode[]>();
  for (const node of nodes) {
    const group = byDevice.get(node.device_id);
    if (group) {
      group.push(node);
    } else {
      byDevice.set(node.device_id, [node]);
    }
  }
  return [...byDevice.entries()]
    .map(([deviceId, deviceNodes]) => ({
      deviceId,
      deviceName: deviceNodes[0]?.device_name ?? '',
      isLocal: deviceNodes.some((node) => node.is_local),
      nodes: deviceNodes,
    }))
    .sort((left, right) => {
      if (left.isLocal !== right.isLocal) {
        return left.isLocal ? -1 : 1;
      }
      const byName = left.deviceName.localeCompare(right.deviceName);
      return byName !== 0 ? byName : left.deviceId.localeCompare(right.deviceId);
    })
    .map((group, index) => ({
      ...group,
      colorIndex: (index % DEVICE_COLOR_COUNT) + 1,
    }));
}

export function deviceColorClass(colorIndex: number | undefined): string {
  return colorIndex ? `device-color-${colorIndex}` : '';
}
