import type { PermissionKind, PermissionStatus } from '../api/types';

export const PERMISSION_REQUEST_ORDER: PermissionKind[] = [
  'input_monitoring',
  'accessibility',
  'screen_recording',
];

export function nextPermissionToFix(
  statuses: PermissionStatus[],
): PermissionStatus | null {
  for (const kind of PERMISSION_REQUEST_ORDER) {
    const status = statuses.find((item) => item.kind === kind);
    if (status && !status.granted) {
      return status;
    }
  }
  return statuses.find((status) => !status.granted) ?? null;
}
