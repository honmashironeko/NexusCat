import { useMemo } from 'react';
import type { DeviceInfo, DisplayNode, SessionSnapshot } from '../api/types';
import { activeNodes, currentNodeForSession } from '../domain/group';
import { fallbackDisplayNode } from '../domain/layout';
import { useApp } from '../state/appStore';
import { selectSurface, type AppSurface } from '../state/reducer';

export interface SessionView {
  surface: AppSurface;
  session: SessionSnapshot | null;
  peer: DeviceInfo | null;
  currentNode: DisplayNode | null;
  currentDeviceName: string | null;
  activeDeviceId: string | null;
  activeDisplayName: string | null;
  inputOwnerName: string | null;
  connected: boolean;
  unreadCount: number;
}

const ACTIVE_SESSION_STATES = new Set([
  'connected',
  'kvm',
  'aggregating',
  'reconnecting',
]);

export function useSession(): SessionView {
  const { state } = useApp();

  return useMemo(() => {
    const session = state.session;
    const peer = session?.peer ?? null;
    const activeNode = currentNodeForSession(state.layout, session);
    const activeDeviceId =
      activeNode?.device_id ??
      session?.current_display_device ??
      peer?.id ??
      activeNodes(state.layout).find((node) => node.is_primary)?.device_id ??
      null;
    const currentNode = activeNode ?? fallbackDisplayNode(activeNodes(state.layout), activeDeviceId);
    const currentDeviceName =
      currentNode?.device_name ??
      peer?.name ??
      state.devices.find((device) => device.info.id === activeDeviceId)?.info.name ??
      null;
    let inputOwnerName: string | null = null;
    if (session?.input_owner === 'local') {
      inputOwnerName = currentNode?.is_local
        ? currentNode.device_name
        : activeNodes(state.layout).find((node) => node.is_local)?.device_name ?? null;
    } else if (session?.input_owner && typeof session.input_owner === 'object') {
      const remoteId = session.input_owner.remote.device_id;
      inputOwnerName =
        activeNodes(state.layout).find((node) => node.device_id === remoteId)?.device_name ??
        state.devices.find((device) => device.info.id === remoteId)?.info.name ??
        null;
    }
    const connected = Boolean(
      session && (ACTIVE_SESSION_STATES.has(session.state) || peer !== null),
    );
    return {
      surface: selectSurface(state),
      session,
      peer,
      currentNode,
      currentDeviceName,
      activeDeviceId,
      activeDisplayName:
        currentNode?.display_name.trim() || currentDeviceName,
      inputOwnerName,
      connected,
      unreadCount: state.notifications.reduce(
        (count, notification) => (notification.read ? count : count + 1),
        0,
      ),
    };
  }, [state]);
}
