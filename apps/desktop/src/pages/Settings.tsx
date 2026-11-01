import {
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from 'react';
import { api } from '../api/client';
import type {
  AppSettings,
  InputServiceStatus,
  PermissionKind,
  ShortcutSettings,
} from '../api/types';
import { Icon, type IconName } from '../components/Icon';
import { Modal } from '../components/Modal';
import { ShortcutRecorder } from '../components/ShortcutRecorder';
import { Toggle } from '../components/Toggle';
import { permissionKindKey } from '../domain/labels';
import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_ACTIONS,
  type ShortcutAction,
} from '../domain/shortcuts';
import { useApp } from '../state/appStore';
import type { TranslationKey } from '../i18n';
import { Updates } from './Updates';

type SettingsSection =
  | 'device'
  | 'operation'
  | 'shortcuts'
  | 'sharing'
  | 'permissions'
  | 'updates';

const SETTINGS_SECTIONS: Array<{
  id: SettingsSection;
  icon: IconName;
  labelKey: TranslationKey;
}> = [
  { id: 'device', icon: 'laptop', labelKey: 'settings.deviceSection' },
  { id: 'operation', icon: 'hand', labelKey: 'settings.operationSection' },
  { id: 'shortcuts', icon: 'settings', labelKey: 'settings.shortcutsSection' },
  { id: 'sharing', icon: 'globe', labelKey: 'settings.sharedSection' },
  { id: 'permissions', icon: 'shield', labelKey: 'settings.permissionSection' },
  { id: 'updates', icon: 'updates', labelKey: 'settings.updatesSection' },
];

function SettingsCard({
  title,
  scope,
  icon,
  children,
}: {
  title: string;
  scope: 'local' | 'shared';
  icon: IconName;
  children: ReactNode;
}) {
  const { t } = useApp();
  return (
    <section className="settings-card">
      <header className="settings-card-head">
        <div className="settings-card-heading">
          <span className="settings-card-icon" aria-hidden="true">
            <Icon name={icon} size={15} />
          </span>
          <h2>{title}</h2>
        </div>
        <span className={scope === 'local' ? 'scope-tag is-local' : 'scope-tag is-shared'}>
          {scope === 'local' ? t('settings.localTag') : t('settings.sharedTag')}
        </span>
      </header>
      <div className="settings-card-body">{children}</div>
    </section>
  );
}

export function Settings() {
  const {
    state,
    t,
    notify,
    reportError,
    saveSettings,
    requestPermission,
    openPermissionSettings,
    openOnboarding,
  } = useApp();
  const settings = state.settings;
  const syncState = state.sharedConfig.state;
  const syncActive = syncState === 'synced';
  const syncBusy = syncState === 'syncing';
  const [section, setSection] = useState<SettingsSection>('device');
  const [deviceName, setDeviceName] = useState(settings.device_name);
  const [nameError, setNameError] = useState<string | null>(null);
  const [nameBusy, setNameBusy] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [syncActionBusy, setSyncActionBusy] = useState(false);
  const [permissionBusy, setPermissionBusy] = useState<PermissionKind | null>(null);
  const [recordingShortcut, setRecordingShortcut] = useState<ShortcutAction | null>(null);
  const [shortcutSaving, setShortcutSaving] = useState(false);
  const [updateOpen, setUpdateOpen] = useState(false);
  const [inputService, setInputService] = useState<InputServiceStatus | null>(null);
  const [inputServiceBusy, setInputServiceBusy] = useState<'install' | 'uninstall' | null>(
    null,
  );
  const captureActive = useRef(false);
  const nameErrorId = useId();
  const deviceNameId = useId();

  const syncNow = async () => {
    if (syncActionBusy) {
      return;
    }
    setSyncActionBusy(true);
    try {
      await api.syncSharedConfig();
    } catch (error) {
      reportError(error);
    } finally {
      setSyncActionBusy(false);
    }
  };

  const installInputService = async () => {
    if (inputServiceBusy) {
      return;
    }
    setInputServiceBusy('install');
    try {
      await api.installInputService();
      notify(
        'success',
        t('settings.inputServiceSection'),
        t('settings.inputServiceInstalledToast'),
      );
      setInputService(await api.getInputServiceStatus());
    } catch (error) {
      reportError(error, t('settings.inputServiceInstallFailed'));
    } finally {
      setInputServiceBusy(null);
    }
  };

  const uninstallInputService = async () => {
    if (inputServiceBusy) {
      return;
    }
    setInputServiceBusy('uninstall');
    try {
      await api.uninstallInputService();
      notify(
        'success',
        t('settings.inputServiceSection'),
        t('settings.inputServiceUninstalledToast'),
      );
      setInputService(await api.getInputServiceStatus());
    } catch (error) {
      reportError(error, t('settings.inputServiceUninstallFailed'));
    } finally {
      setInputServiceBusy(null);
    }
  };

  useEffect(() => {
    setDeviceName(settings.device_name);
  }, [settings.device_name]);

  useEffect(() => {
    if (state.updateRequested > 0) {
      setUpdateOpen(true);
    }
  }, [state.updateRequested]);

  useEffect(() => {
    let disposed = false;
    const load = async () => {
      try {
        const status = await api.getInputServiceStatus();
        if (!disposed) {
          setInputService(status);
        }
      } catch {
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 3_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    const shouldCapture = recordingShortcut !== null;
    if (shouldCapture && !captureActive.current) {
      captureActive.current = true;
      void api.setShortcutCapture(true).catch((error) => reportError(error));
      return;
    }
    if (!shouldCapture && !shortcutSaving && captureActive.current) {
      captureActive.current = false;
      void api.setShortcutCapture(false).catch((error) => reportError(error));
    }
  }, [recordingShortcut, shortcutSaving, reportError]);

  useEffect(() => {
    return () => {
      if (captureActive.current) {
        void api.setShortcutCapture(false).catch(() => undefined);
      }
    };
  }, []);

  const nameChanged = deviceName.trim() !== settings.device_name;
  const inputServiceStatus = !inputService
    ? { label: t('common.loading'), className: 'chip' }
    : !inputService.installed
      ? { label: t('settings.inputServiceMissing'), className: 'chip chip-warning' }
      : inputService.needs_update
        ? {
            label: t('settings.inputServiceNeedsUpdate'),
            className: 'chip chip-warning',
          }
        : inputService.connected
          ? {
              label: t('settings.inputServiceConnected'),
              className: 'chip chip-success',
            }
          : !inputService.running
            ? {
                label: t('settings.inputServiceStopped'),
                className: 'chip chip-warning',
              }
            : {
                label: t('settings.inputServiceWaiting'),
                className: 'chip chip-warning',
              };

  const persist = (patch: Partial<AppSettings>) => {
    void saveSettings(patch);
  };

  const saveDeviceName = async (event?: FormEvent) => {
    event?.preventDefault();
    const value = deviceName.trim();
    if (!value) {
      setNameError(t('settings.deviceNameError'));
      return;
    }
    if (value === settings.device_name) {
      setDeviceName(value);
      setNameError(null);
      return;
    }
    setNameBusy(true);
    const saved = await saveSettings({ device_name: value });
    if (saved) {
      setDeviceName(saved.device_name);
      setNameError(null);
    } else {
      setDeviceName(settings.device_name);
    }
    setNameBusy(false);
  };

  const handleAutostart = async (enabled: boolean) => {
    if (autostartBusy) {
      return;
    }
    setAutostartBusy(true);
    try {
      await api.setAutostart(enabled);
      await saveSettings({ autostart: enabled });
    } catch (error) {
      reportError(error, t('settings.autostartError'));
    } finally {
      setAutostartBusy(false);
    }
  };

  const handlePermission = async (kind: PermissionKind) => {
    setPermissionBusy(kind);
    try {
      await requestPermission(kind);
    } finally {
      setPermissionBusy(null);
    }
  };

  const saveShortcut = async (action: ShortcutAction, value: string): Promise<boolean> => {
    if (shortcutSaving) {
      return false;
    }
    setShortcutSaving(true);
    setRecordingShortcut(null);
    const next: ShortcutSettings = { ...settings.shortcuts, [action]: value };
    const saved = await saveSettings({ shortcuts: next });
    setShortcutSaving(false);
    return Boolean(saved);
  };

  const resetShortcuts = async () => {
    if (shortcutSaving) {
      return;
    }
    setShortcutSaving(true);
    setRecordingShortcut(null);
    const saved = await saveSettings({ shortcuts: { ...DEFAULT_SHORTCUTS } });
    setShortcutSaving(false);
    if (saved) {
      notify('success', t('settings.shortcutResetDone'));
    }
  };

  const takenShortcuts = (action: ShortcutAction): string[] =>
    SHORTCUT_ACTIONS.filter((item) => item !== action).map(
      (item) => settings.shortcuts[item],
    );

  const syncStatusLabel = syncActive
    ? t('settings.syncStatusSyncedCount', {
        count: state.sharedConfig.synced_devices,
      })
    : syncBusy
      ? t('settings.syncStatusSyncing', {
          synced: state.sharedConfig.synced_devices,
          online: state.sharedConfig.online_devices,
        })
      : t('settings.syncStatus');

  return (
    <div className="page settings-page">
      <header className="view-heading settings-heading">
        <h1>{t('settings.title')}</h1>
      </header>

      <div className="settings-layout">
        <nav className="settings-nav" aria-label={t('settings.navAria')}>
          {SETTINGS_SECTIONS.map((item) => {
            const active = section === item.id;
            return (
              <button
                key={item.id}
                type="button"
                className={active ? 'settings-nav-item is-active' : 'settings-nav-item'}
                aria-current={active ? 'page' : undefined}
                onClick={() => setSection(item.id)}
              >
                <Icon name={item.icon} size={14} />
                <span>{t(item.labelKey)}</span>
              </button>
            );
          })}
        </nav>

        <div className="settings-panel">
          {section === 'device' ? (
            <SettingsCard
              title={t('settings.deviceSection')}
              scope="local"
              icon="laptop"
            >
              <div className="setting-row setting-row-stack">
                <label className="setting-label" htmlFor={deviceNameId}>
                  {t('settings.deviceName')}
                </label>
                <form className="inline-field" onSubmit={saveDeviceName}>
                  <input
                    id={deviceNameId}
                    className={nameError ? 'text-input has-error' : 'text-input'}
                    value={deviceName}
                    onChange={(event) => {
                      setDeviceName(event.target.value);
                      setNameError(null);
                    }}
                    placeholder={t('settings.deviceNamePlaceholder')}
                    aria-invalid={Boolean(nameError)}
                    aria-describedby={nameError ? nameErrorId : undefined}
                    autoComplete="off"
                    maxLength={64}
                  />
                  <button
                    type="submit"
                    className="btn compact"
                    disabled={!nameChanged || nameBusy}
                  >
                    {nameBusy ? t('common.busy') : t('common.save')}
                  </button>
                </form>
                {nameError ? (
                  <p className="inline-error" id={nameErrorId} role="alert">
                    {nameError}
                  </p>
                ) : null}
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.autostart')}</span>
                  <span className="setting-help">{t('settings.autostartHelp')}</span>
                </div>
                <Toggle
                  checked={settings.autostart}
                  label={t('settings.autostart')}
                  disabled={autostartBusy}
                  onChange={(enabled) => void handleAutostart(enabled)}
                />
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.fileTransfer')}</span>
                  <span className="setting-help">{t('settings.fileTransferHelp')}</span>
                </div>
                <Toggle
                  checked={settings.file_transfer_enabled}
                  label={t('settings.fileTransfer')}
                  onChange={(enabled) => persist({ file_transfer_enabled: enabled })}
                />
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.rerunOnboarding')}</span>
                </div>
                <button type="button" className="btn compact" onClick={openOnboarding}>
                  {t('common.open')}
                </button>
              </div>
            </SettingsCard>
          ) : null}

          {section === 'operation' ? (
            <SettingsCard
              title={t('settings.operationSection')}
              scope="local"
              icon="hand"
            >
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.edgeEnabled')}</span>
                </div>
                <Toggle
                  checked={settings.edge_enabled}
                  label={t('settings.edgeEnabled')}
                  onChange={(enabled) => persist({ edge_enabled: enabled })}
                />
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.edgeInset')}</span>
                </div>
                <Toggle
                  checked={settings.edge_inset}
                  label={t('settings.edgeInset')}
                  onChange={(enabled) => persist({ edge_inset: enabled })}
                />
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.stageOnPrimary')}</span>
                </div>
                <Toggle
                  checked={settings.stage_on_primary}
                  label={t('settings.stageOnPrimary')}
                  onChange={(enabled) => persist({ stage_on_primary: enabled })}
                />
              </div>
            </SettingsCard>
          ) : null}

          {section === 'shortcuts' ? (
            <SettingsCard
              title={t('settings.shortcutsSection')}
              scope="local"
              icon="settings"
            >
              <div className="setting-row shortcut-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.shortcutToggleMode')}</span>
                  <span className="setting-help">{t('settings.shortcutToggleModeHelp')}</span>
                </div>
                <ShortcutRecorder
                  value={settings.shortcuts.toggle_mode}
                  label={t('settings.shortcutToggleMode')}
                  taken={takenShortcuts('toggle_mode')}
                  recording={recordingShortcut === 'toggle_mode'}
                  disabled={shortcutSaving}
                  onRecordingChange={(active) =>
                    setRecordingShortcut(active ? 'toggle_mode' : null)
                  }
                  onCommit={(value) => saveShortcut('toggle_mode', value)}
                />
              </div>
              <div className="setting-row shortcut-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.shortcutToggleStage')}</span>
                  <span className="setting-help">{t('settings.shortcutToggleStageHelp')}</span>
                </div>
                <ShortcutRecorder
                  value={settings.shortcuts.toggle_stage}
                  label={t('settings.shortcutToggleStage')}
                  taken={takenShortcuts('toggle_stage')}
                  recording={recordingShortcut === 'toggle_stage'}
                  disabled={shortcutSaving}
                  onRecordingChange={(active) =>
                    setRecordingShortcut(active ? 'toggle_stage' : null)
                  }
                  onCommit={(value) => saveShortcut('toggle_stage', value)}
                />
              </div>
              <div className="setting-row shortcut-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.shortcutOpenCalibration')}</span>
                  <span className="setting-help">
                    {t('settings.shortcutOpenCalibrationHelp')}
                  </span>
                </div>
                <ShortcutRecorder
                  value={settings.shortcuts.open_calibration}
                  label={t('settings.shortcutOpenCalibration')}
                  taken={takenShortcuts('open_calibration')}
                  recording={recordingShortcut === 'open_calibration'}
                  disabled={shortcutSaving}
                  onRecordingChange={(active) =>
                    setRecordingShortcut(active ? 'open_calibration' : null)
                  }
                  onCommit={(value) => saveShortcut('open_calibration', value)}
                />
              </div>
              <div className="setting-row shortcut-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.shortcutPauseRemote')}</span>
                  <span className="setting-help">{t('settings.shortcutPauseRemoteHelp')}</span>
                </div>
                <ShortcutRecorder
                  value={settings.shortcuts.pause_remote}
                  label={t('settings.shortcutPauseRemote')}
                  taken={takenShortcuts('pause_remote')}
                  recording={recordingShortcut === 'pause_remote'}
                  disabled={shortcutSaving}
                  onRecordingChange={(active) =>
                    setRecordingShortcut(active ? 'pause_remote' : null)
                  }
                  onCommit={(value) => saveShortcut('pause_remote', value)}
                />
              </div>
              <div className="setting-row shortcut-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.shortcutToggleEdge')}</span>
                  <span className="setting-help">{t('settings.shortcutToggleEdgeHelp')}</span>
                </div>
                <ShortcutRecorder
                  value={settings.shortcuts.toggle_edge}
                  label={t('settings.shortcutToggleEdge')}
                  taken={takenShortcuts('toggle_edge')}
                  recording={recordingShortcut === 'toggle_edge'}
                  disabled={shortcutSaving}
                  onRecordingChange={(active) =>
                    setRecordingShortcut(active ? 'toggle_edge' : null)
                  }
                  onCommit={(value) => saveShortcut('toggle_edge', value)}
                />
              </div>
              <div className="setting-row">
                <button
                  type="button"
                  className="btn compact"
                  disabled={shortcutSaving}
                  onClick={() => void resetShortcuts()}
                >
                  {shortcutSaving ? t('common.busy') : t('settings.shortcutResetAll')}
                </button>
              </div>
            </SettingsCard>
          ) : null}

          {section === 'sharing' ? (
            <SettingsCard
              title={t('settings.sharedSection')}
              scope="shared"
              icon="globe"
            >
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.audio')}</span>
                  <span className="setting-help">{t('settings.audioHelp')}</span>
                </div>
                <Toggle
                  checked={settings.audio_enabled}
                  label={t('settings.audio')}
                  onChange={(enabled) => persist({ audio_enabled: enabled })}
                />
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.clipboard')}</span>
                  <span className="setting-help">{t('settings.clipboardHelp')}</span>
                </div>
                <Toggle
                  checked={settings.clipboard_enabled}
                  label={t('settings.clipboard')}
                  onChange={(enabled) => persist({ clipboard_enabled: enabled })}
                />
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.syncNow')}</span>
                  <span className="setting-help">{syncStatusLabel}</span>
                </div>
                <button
                  type="button"
                  className="btn compact"
                  disabled={syncActionBusy || state.sharedConfig.online_devices === 0}
                  onClick={() => void syncNow()}
                >
                  {syncActionBusy ? (
                    <span className="spinner" aria-hidden="true" />
                  ) : (
                    <Icon name="refresh" size={12} />
                  )}
                  {syncActionBusy ? t('common.busy') : t('settings.syncNow')}
                </button>
              </div>
            </SettingsCard>
          ) : null}

          {section === 'permissions' ? (
            <>
              <SettingsCard
                title={t('settings.permissionSection')}
                scope="local"
                icon="shield"
              >
                {state.permissions.statuses.length === 0 ? (
                  <p className="settings-empty">{t('settings.permissionNone')}</p>
                ) : (
                  state.permissions.statuses.map((status) => {
                    const label = t(permissionKindKey(status.kind));
                    return (
                      <div className="setting-row permission-row" key={status.kind}>
                        <div className="setting-copy">
                          <span className="setting-label">{label}</span>
                          <span
                            className={
                              status.granted
                                ? 'permission-state granted'
                                : 'permission-state missing'
                            }
                          >
                            <Icon name={status.granted ? 'check' : 'warning'} size={12} />
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
                              aria-label={t('settings.permissionRequestAria', { name: label })}
                              onClick={() => void handlePermission(status.kind)}
                            >
                              {permissionBusy === status.kind ? (
                                <span className="spinner" aria-hidden="true" />
                              ) : (
                                <Icon name="shield" size={12} />
                              )}
                              {t('settings.permissionRequest')}
                            </button>
                          </span>
                        ) : null}
                      </div>
                    );
                  })
                )}
              </SettingsCard>

              {state.bootstrap?.platform === 'windows' ? (
                <SettingsCard
                  title={t('settings.inputServiceSection')}
                  scope="local"
                  icon="shield"
                >
                  <div className="setting-row">
                    <div className="setting-copy">
                      <span className="setting-label">{t('settings.inputServiceLabel')}</span>
                      <span className="setting-help">{t('settings.inputServiceHelp')}</span>
                      {inputService?.version ? (
                        <span className="setting-help">
                          {t('settings.inputServiceVersion', {
                            version: inputService.version,
                          })}
                        </span>
                      ) : null}
                    </div>
                    <span className={inputServiceStatus.className}>
                      {inputServiceStatus.label}
                    </span>
                  </div>
                  <div className="setting-row">
                    <div className="head-actions">
                      {inputService?.installed ? (
                        <button
                          type="button"
                          className="btn compact"
                          disabled={inputServiceBusy !== null}
                          onClick={() => void uninstallInputService()}
                        >
                          {inputServiceBusy === 'uninstall' ? (
                            <span className="spinner" aria-hidden="true" />
                          ) : (
                            <Icon name="unlink" size={12} />
                          )}
                          {t('settings.inputServiceUninstall')}
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="btn primary compact"
                        disabled={inputServiceBusy !== null}
                        onClick={() => void installInputService()}
                      >
                        {inputServiceBusy === 'install' ? (
                          <span className="spinner" aria-hidden="true" />
                        ) : (
                          <Icon name="shield" size={12} />
                        )}
                        {inputService?.installed
                          ? t('settings.inputServiceReinstall')
                          : t('settings.inputServiceInstall')}
                      </button>
                    </div>
                  </div>
                </SettingsCard>
              ) : null}
            </>
          ) : null}

          {section === 'updates' ? (
            <SettingsCard
              title={t('settings.updatesSection')}
              scope="shared"
              icon="updates"
            >
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.updateAutoAccept')}</span>
                  <span className="setting-help">{t('settings.updateAutoAcceptHelp')}</span>
                </div>
                <Toggle
                  checked={settings.update_auto_accept}
                  label={t('settings.updateAutoAccept')}
                  onChange={(enabled) => persist({ update_auto_accept: enabled })}
                />
              </div>
              <div className="setting-row">
                <div className="setting-copy">
                  <span className="setting-label">{t('settings.version')}</span>
                  <span className="setting-help">
                    {t('settings.currentVersion')} · v{state.bootstrap?.version ?? '1.0.0'}
                  </span>
                </div>
                <button
                  type="button"
                  className="btn primary compact"
                  onClick={() => setUpdateOpen(true)}
                >
                  <Icon name="updates" size={13} />
                  {t('settings.pushUpdate')}
                </button>
              </div>
            </SettingsCard>
          ) : null}
        </div>
      </div>

      <Modal
        open={updateOpen}
        title={t('settings.updateSection')}
        size="lg"
        onClose={() => setUpdateOpen(false)}
      >
        <Updates embedded />
      </Modal>
    </div>
  );
}
