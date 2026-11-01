import { Fragment, useEffect, useId, useState } from 'react';
import { shortcutFromKeyEvent } from '../domain/shortcuts';
import type { TranslationKey } from '../i18n';
import { useApp } from '../state/appStore';

const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta']);

function isMacPlatform(): boolean {
  return typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent);
}

function displayPart(part: string): string {
  if (part === 'Super') {
    return isMacPlatform() ? '⌘' : 'Win';
  }
  return part;
}

export function ShortcutKeys({ value }: { value: string }) {
  if (!value.trim()) {
    return null;
  }
  const parts = value
    .split('+')
    .map((part) => part.trim())
    .filter(Boolean);
  return (
    <span className="shortcut-keys">
      {parts.map((part, index) => (
        <Fragment key={`${part}-${index}`}>
          {index > 0 ? <span className="shortcut-plus">+</span> : null}
          <kbd>{displayPart(part)}</kbd>
        </Fragment>
      ))}
    </span>
  );
}

export function ShortcutRecorder({
  value,
  label,
  taken,
  recording,
  disabled,
  onRecordingChange,
  onCommit,
}: {
  value: string;
  label: string;
  taken: string[];
  recording: boolean;
  disabled: boolean;
  onRecordingChange: (recording: boolean) => void;
  onCommit: (value: string) => Promise<boolean>;
}) {
  const { t } = useApp();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<TranslationKey | null>(null);
  const errorId = useId();

  useEffect(() => {
    if (!recording) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) {
        return;
      }
      if (event.key === 'Escape') {
        onRecordingChange(false);
        setError(null);
        return;
      }
      if (MODIFIER_KEYS.has(event.key)) {
        return;
      }
      const shortcut = shortcutFromKeyEvent(event);
      if (!shortcut) {
        setError('settings.shortcutNeedModifier');
        return;
      }
      if (taken.some((item) => item.trim().toLowerCase() === shortcut.toLowerCase())) {
        setError('settings.shortcutDuplicate');
        return;
      }
      if (shortcut === value) {
        onRecordingChange(false);
        setError(null);
        return;
      }
      setError(null);
      setSaving(true);
      void onCommit(shortcut).then((saved) => {
        setSaving(false);
        if (!saved) {
          setError('settings.shortcutSaveError');
        }
      });
    };
    window.addEventListener('keydown', handleKeyDown, true);
    return () => window.removeEventListener('keydown', handleKeyDown, true);
  }, [onCommit, onRecordingChange, recording, t, taken, value]);

  const clear = () => {
    if (saving || disabled) {
      return;
    }
    setError(null);
    setSaving(true);
    void onCommit('').then((saved) => {
      setSaving(false);
      if (!saved) {
        setError('settings.shortcutSaveError');
      }
    });
  };

  return (
    <div className="shortcut-recorder">
      <div className="shortcut-recorder-row">
        <button
          type="button"
          className={recording ? 'shortcut-button is-recording' : 'shortcut-button'}
          aria-label={`${label} · ${t('settings.shortcutRecordAria')}`}
          aria-describedby={error ? errorId : undefined}
          disabled={saving || disabled}
          onClick={() => {
            onRecordingChange(!recording);
            setError(null);
          }}
        >
          {recording ? (
            <span>{t('settings.shortcutRecording')}</span>
          ) : value ? (
            <ShortcutKeys value={value} />
          ) : (
            <span className="shortcut-unset">{t('settings.shortcutUnset')}</span>
          )}
        </button>
        {value && !recording ? (
          <button
            type="button"
            className="text-button shortcut-clear"
            disabled={saving}
            onClick={clear}
          >
            {t('settings.shortcutClear')}
          </button>
        ) : null}
        {saving ? <span className="spinner" aria-hidden="true" /> : null}
      </div>
      {recording ? (
        <span className="shortcut-hint">{t('settings.shortcutPressHint')}</span>
      ) : null}
      {error ? (
        <span className="shortcut-error" id={errorId} role="alert">
          {t(error)}
        </span>
      ) : null}
    </div>
  );
}
