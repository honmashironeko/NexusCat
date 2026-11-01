import { useState } from 'react';
import { api } from '../api/client';
import { errorMessage } from '../api/errors';
import type { PermissionKind } from '../api/types';
import { Icon } from '../components/Icon';
import { LanguageToggle } from '../components/LanguageToggle';
import { Logo } from '../components/Logo';
import { ThemeToggle } from '../components/ThemeToggle';
import {
  permissionKindKey,
  platformLabelKey,
} from '../domain/labels';
import { useApp } from '../state/appStore';

const TOTAL_STEPS = 3;

export function Onboarding() {
  const {
    state,
    t,
    saveSettings,
    completeOnboarding,
    requestPermission,
    openPermissionSettings,
    notify,
  } = useApp();
  const [step, setStep] = useState(0);
  const [deviceName, setDeviceName] = useState(state.settings.device_name);
  const [nameError, setNameError] = useState<string | null>(null);
  const [finishing, setFinishing] = useState(false);
  const [permissionBusy, setPermissionBusy] = useState<PermissionKind | null>(null);
  const hasMissingPermissions = state.permissions.statuses.some((status) => !status.granted);

  const validateName = (): boolean => {
    if (!deviceName.trim()) {
      setNameError(t('onboarding.deviceNameError'));
      return false;
    }
    setNameError(null);
    return true;
  };

  const handleNext = () => {
    if (step === 0 && !validateName()) {
      return;
    }
    if (step < TOTAL_STEPS - 1) {
      setStep((current) => current + 1);
    }
  };

  const handleFinish = async () => {
    if (!validateName() || finishing) {
      return;
    }
    setFinishing(true);
    const trimmedName = deviceName.trim();
    if (trimmedName !== state.settings.device_name) {
      const saved = await saveSettings({ device_name: trimmedName });
      if (!saved) {
        setFinishing(false);
        return;
      }
    }
    try {
      await api.startDiscovery();
      await api.probeDevices();
    } catch (error) {
      notify(
        'warning',
        t('onboarding.startDiscoveryError'),
        errorMessage(error) || undefined,
      );
    }
    completeOnboarding();
    setFinishing(false);
  };

  const selectMode = async (mode: 'kvm' | 'aggregation') => {
    if (state.settings.mode === mode) {
      return;
    }
    await saveSettings({ mode });
  };

  const handlePermission = async (kind: PermissionKind) => {
    setPermissionBusy(kind);
    try {
      await requestPermission(kind);
    } finally {
      setPermissionBusy(null);
    }
  };

  return (
    <div className="onboarding-shell">
      <aside className="onboarding-brand">
        <div className="onboarding-brand-top">
          <Logo size={64} />
          <div>
            <span className="eyebrow">NexusCat</span>
            <strong>v{state.bootstrap?.version ?? '1.0.0'}</strong>
          </div>
        </div>
        <div className="onboarding-art" aria-hidden="true">
          <div className="art-node art-local">
            <Icon name="monitor" size={18} />
            <span>{deviceName.trim() || t('common.local')}</span>
          </div>
          <div className="art-link">
            <span />
            <Icon name="link" size={18} />
            <span />
          </div>
          <div className="art-node art-remote">
            <Icon name="laptop" size={18} />
            <span>NexusCat</span>
          </div>
        </div>
        <div className="onboarding-brand-copy">
          <h1>{t('onboarding.welcomeTitle')}</h1>
          <p>{t('onboarding.welcomeBody')}</p>
        </div>
        <div className="onboarding-brand-foot">
          <span>{t(platformLabelKey(state.bootstrap?.platform ?? 'windows'))}</span>
          <span>LAN</span>
        </div>
      </aside>

      <main className="onboarding-panel">
        <div className="onboarding-topbar">
          <span className="step-label">
            {t('onboarding.step', { current: step + 1, total: TOTAL_STEPS })}
          </span>
          <div className="onboarding-preferences">
            <LanguageToggle compact />
            <ThemeToggle variant="compact" />
          </div>
        </div>

        <div className="step-track" aria-hidden="true">
          {Array.from({ length: TOTAL_STEPS }, (_, index) => (
            <span
              key={index}
              className={index <= step ? 'step-dot is-active' : 'step-dot'}
            />
          ))}
        </div>

        <div className="onboarding-content">
          {step === 0 ? (
            <section className="onboarding-step">
              <span className="onboarding-kicker">{t('settings.general')}</span>
              <h2>{t('onboarding.deviceName')}</h2>
              <p>{t('onboarding.deviceNameHelp')}</p>
              <label className="field-label sr-only" htmlFor="onboarding-device-name">
                {t('onboarding.deviceName')}
              </label>
              <input
                id="onboarding-device-name"
                className={nameError ? 'text-input large has-error' : 'text-input large'}
                value={deviceName}
                onChange={(event) => {
                  setDeviceName(event.target.value);
                  setNameError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    handleNext();
                  }
                }}
                placeholder={t('settings.deviceNamePlaceholder')}
                autoComplete="off"
                maxLength={64}
                autoFocus
                aria-invalid={Boolean(nameError)}
                aria-describedby={nameError ? 'onboarding-name-error' : undefined}
              />
              {nameError ? (
                <p className="inline-error" id="onboarding-name-error" role="alert">
                  {nameError}
                </p>
              ) : null}
            </section>
          ) : null}

          {step === 1 ? (
            <section className="onboarding-step">
              <span className="onboarding-kicker">{t('overview.activeMode')}</span>
              <h2>{t('onboarding.modeTitle')}</h2>
              <div className="mode-explainer">
                <button
                  type="button"
                  className={
                    state.settings.mode === 'kvm'
                      ? 'mode-card is-active'
                      : 'mode-card'
                  }
                  aria-pressed={state.settings.mode === 'kvm'}
                  onClick={() => void selectMode('kvm')}
                >
                  <span className="mode-icon" aria-hidden="true">
                    <Icon name="devices" size={22} />
                  </span>
                  <div>
                    <h3>{t('onboarding.modeKvm')}</h3>
                    <p>{t('onboarding.modeKvmBody')}</p>
                  </div>
                  <span className="mode-tag">
                    {state.settings.mode === 'kvm'
                      ? t('onboarding.modeSelected')
                      : t('session.mode.kvm')}
                  </span>
                </button>
                <button
                  type="button"
                  className={
                    state.settings.mode === 'aggregation'
                      ? 'mode-card is-active'
                      : 'mode-card'
                  }
                  aria-pressed={state.settings.mode === 'aggregation'}
                  onClick={() => void selectMode('aggregation')}
                >
                  <span className="mode-icon" aria-hidden="true">
                    <Icon name="spark" size={22} />
                  </span>
                  <div>
                    <h3>{t('onboarding.modeAggregation')}</h3>
                    <p>{t('onboarding.modeAggregationBody')}</p>
                  </div>
                  <span className="mode-tag">
                    {state.settings.mode === 'aggregation'
                      ? t('onboarding.modeSelected')
                      : t('session.mode.aggregation')}
                  </span>
                </button>
              </div>
              <p className="onboarding-hint">{t('onboarding.modeSwitchHint')}</p>
            </section>
          ) : null}

          {step === 2 ? (
            <section className="onboarding-step">
              <span className="onboarding-kicker">{t('settings.permissions')}</span>
              <h2>{t('onboarding.permissionsTitle')}</h2>
              <p>{t('onboarding.permissionsBody')}</p>
              {state.permissions.statuses.length === 0 ? (
                <div className="permission-complete">
                  <Icon name="check" size={18} />
                  {t('permissions.none')}
                </div>
              ) : (
                <ul className="onboarding-permission-list">
                  {state.permissions.statuses.map((status) => {
                    const label = t(permissionKindKey(status.kind));
                    return (
                      <li key={status.kind}>
                        <span className="permission-symbol" aria-hidden="true">
                          <Icon name={status.granted ? 'check' : 'shield'} size={15} />
                        </span>
                        <div>
                          <strong>{label}</strong>
                          <span className={status.granted ? 'permission-state granted' : 'permission-state missing'}>
                            {status.granted
                              ? t('settings.permissionGranted')
                              : t('settings.permissionMissing')}
                          </span>
                        </div>
                        {!status.granted ? (
                          <span className="permission-actions">
                            <button
                              type="button"
                              className="btn ghost compact"
                              aria-label={t('permissions.openSettingsAria', { name: label })}
                              onClick={() => void openPermissionSettings(status.kind)}
                            >
                              {t('permissions.openSettings')}
                            </button>
                            <button
                              type="button"
                              className="btn compact"
                              disabled={permissionBusy === status.kind}
                              aria-label={t('permissions.requestAria', { name: label })}
                              onClick={() => void handlePermission(status.kind)}
                            >
                              {permissionBusy === status.kind ? (
                                <span className="spinner" aria-hidden="true" />
                              ) : null}
                              {t('settings.permissionRequest')}
                            </button>
                          </span>
                        ) : null}
                      </li>
                    );
                  })}
                </ul>
              )}
              {hasMissingPermissions ? (
                <>
                  <p className="onboarding-hint">{t('onboarding.permissionNeeded')}</p>
                  {state.bootstrap?.platform === 'mac_os' ? (
                    <p className="onboarding-hint">
                      {t('onboarding.permissionRestart')}
                    </p>
                  ) : null}
                </>
              ) : (
                <p className="onboarding-hint success-hint">
                  <Icon name="check" size={13} />
                  {t('permissions.allGranted')}
                </p>
              )}
              <p className="onboarding-hint">
                {t('onboarding.edgeReady', {
                  calibration:
                    state.settings.shortcuts.open_calibration ||
                    t('settings.shortcutUnset'),
                  stage:
                    state.settings.shortcuts.toggle_stage ||
                    t('settings.shortcutUnset'),
                })}
              </p>
            </section>
          ) : null}
        </div>

        <footer className="onboarding-footer">
          <button
            type="button"
            className="btn ghost"
            disabled={step === 0 || finishing}
            onClick={() => setStep((current) => Math.max(0, current - 1))}
          >
            {t('common.back')}
          </button>
          {step < TOTAL_STEPS - 1 ? (
            <button type="button" className="btn primary" onClick={handleNext}>
              {t('common.next')}
              <Icon name="arrowRight" size={14} />
            </button>
          ) : (
            <button
              type="button"
              className="btn primary"
              disabled={finishing}
              onClick={() => void handleFinish()}
            >
              {finishing ? (
                <span className="spinner" aria-hidden="true" />
              ) : (
                <Icon name="check" size={14} />
              )}
              {finishing ? t('common.busy') : t('onboarding.finish')}
            </button>
          )}
        </footer>
      </main>
    </div>
  );
}
