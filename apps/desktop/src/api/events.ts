import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { BackendEventMap, BackendEventName } from './types';

export type BackendEventHandler<K extends BackendEventName> = (
  payload: BackendEventMap[K],
) => void;

export function listenBackendEvent<K extends BackendEventName>(
  event: K,
  handler: BackendEventHandler<K>,
): Promise<UnlistenFn> {
  return listen<BackendEventMap[K]>(event, (record) => {
    handler(record.payload);
  });
}
