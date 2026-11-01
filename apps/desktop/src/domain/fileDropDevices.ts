import type { DisplayNode } from '../api/types';

export interface DeviceDropTarget {
  deviceId: string;
  deviceName: string;
  isLocal: boolean;
  nodeId: string;
  displays: string[];
  screenCount: number;
}

export function buildDeviceDropTargets(
  nodes: DisplayNode[],
): DeviceDropTarget[] {
  const groups = new Map<string, DeviceDropTarget>();
  for (const node of nodes) {
    const display = node.display_name.trim();
    const existing = groups.get(node.device_id);
    if (existing) {
      if (display && !existing.displays.includes(display)) {
        existing.displays.push(display);
      }
      existing.screenCount += 1;
      continue;
    }
    groups.set(node.device_id, {
      deviceId: node.device_id,
      deviceName: node.device_name,
      isLocal: node.is_local,
      nodeId: node.id,
      displays: display ? [display] : [],
      screenCount: 1,
    });
  }
  return [...groups.values()].sort(
    (left, right) => Number(left.isLocal) - Number(right.isLocal),
  );
}
