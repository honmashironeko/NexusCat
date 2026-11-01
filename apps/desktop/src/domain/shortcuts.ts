import type { ShortcutSettings } from '../api/types';

export type ShortcutAction = keyof ShortcutSettings;

export const SHORTCUT_ACTIONS: ShortcutAction[] = [
  'toggle_mode',
  'toggle_stage',
  'open_calibration',
  'pause_remote',
  'toggle_edge',
];

export const DEFAULT_SHORTCUTS: ShortcutSettings = {
  toggle_mode: 'Ctrl+Alt+M',
  toggle_stage: 'Ctrl+Alt+F',
  open_calibration: 'Ctrl+Alt+C',
  pause_remote: 'Ctrl+Alt+Space',
  toggle_edge: 'Ctrl+Alt+E',
};

export interface ShortcutKeyEvent {
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

const NAMED_KEYS = new Set([
  'ArrowUp',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
  'Space',
  'Enter',
  'Tab',
  'Backspace',
  'Delete',
  'Home',
  'End',
  'PageUp',
  'PageDown',
  'Insert',
]);

function mainKeyFromCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) {
    return code.slice(3);
  }
  if (/^Digit[0-9]$/.test(code)) {
    return code.slice(5);
  }
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) {
    return code;
  }
  return NAMED_KEYS.has(code) ? code : null;
}

export function shortcutFromKeyEvent(event: ShortcutKeyEvent): string | null {
  if (!event.ctrlKey && !event.altKey && !event.metaKey) {
    return null;
  }
  const key = mainKeyFromCode(event.code);
  if (!key) {
    return null;
  }
  const parts: string[] = [];
  if (event.ctrlKey) {
    parts.push('Ctrl');
  }
  if (event.altKey) {
    parts.push('Alt');
  }
  if (event.shiftKey) {
    parts.push('Shift');
  }
  if (event.metaKey) {
    parts.push('Super');
  }
  parts.push(key);
  return parts.join('+');
}
