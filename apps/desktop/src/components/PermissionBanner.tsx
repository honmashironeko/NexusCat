import { useState } from 'react';
import { permissionKindKey } from '../domain/labels';
import { nextPermissionToFix } from '../domain/permissions';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';

export function PermissionBanner() {
  const { state, t, requestPermission, openPermissionSettings } = useApp();
  const [busy, setBusy] = useState<'request' | 'settings' | null>(null);
  const missing = state.permissions.statuses.filter((status) => !status.granted);
  const target = nextPermissionToFix(state.permissions.statuses);
  if (missing.length === 0) {
    return null;
  }
  const labels = missing.map((status) => t(permissionKindKey(status.kind))).join(' · ');
  const targetLabel = target ? t(permissionKindKey(target.kind)) : '';

  const handleFix = async () => {
    if (!target) {
      return;
    }
    setBusy('request');
    try {
      await requestPermission(target.kind);
    } finally {
      setBusy(null);
    }
  };

  const handleSettings = async () => {
    if (!target) {
      return;
    }
    setBusy('settings');
    try {
      await openPermissionSettings(target.kind);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="permission-banner" role="alert">
      <span className="permission-banner-icon" aria-hidden="true">
        <Icon name="shield" size={16} />
      </span>
      <div className="permission-banner-copy">
        <strong>{t('permissions.bannerTitle')}</strong>
        <span>{t('permissions.bannerBody', { items: labels })}</span>
      </div>
      <span className="permission-banner-actions">
        <button
          type="button"
          className="btn warning compact"
          disabled={busy !== null}
          onClick={() => void handleFix()}
        >
          {busy === 'request' ? t('common.busy') : t('permissions.repair')}
        </button>
        <button
          type="button"
          className="btn ghost compact"
          disabled={busy !== null}
          aria-label={t('permissions.openSettingsAria', { name: targetLabel })}
          onClick={() => void handleSettings()}
        >
          {busy === 'settings' ? t('common.busy') : t('permissions.openSettings')}
        </button>
      </span>
    </div>
  );
}
