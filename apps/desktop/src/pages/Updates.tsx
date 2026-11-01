import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { UpdatePeerState } from '../api/types';
import { Icon } from '../components/Icon';
import { SelectMenu } from '../components/SelectMenu';
import { Toggle } from '../components/Toggle';
import { updateKindKey, updateStateKey } from '../domain/labels';
import { useApp } from '../state/appStore';

function peerStateKey(
  state: UpdatePeerState,
):
  | 'updates.peer.queued'
  | 'updates.peer.connecting'
  | 'updates.peer.transferring'
  | 'updates.peer.done'
  | 'updates.peer.failed' {
  switch (state) {
    case 'queued':
      return 'updates.peer.queued';
    case 'connecting':
      return 'updates.peer.connecting';
    case 'transferring':
      return 'updates.peer.transferring';
    case 'done':
      return 'updates.peer.done';
    default:
      return 'updates.peer.failed';
  }
}

function fileName(path: string): string {
  const normalized = path.replace(/\\/g, '/');
  const segments = normalized.split('/');
  return segments[segments.length - 1] || path;
}

export function Updates({ embedded = false }: { embedded?: boolean }) {
  const {
    state,
    t,
    notify,
    reportError,
    refreshUpdateStatus,
  } = useApp();
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const [pushing, setPushing] = useState(false);
  const [packageKind, setPackageKind] = useState<'auto' | 'installer' | 'portable'>('auto');
  const [forceInstall, setForceInstall] = useState(false);
  const [applying, setApplying] = useState(false);
  const [rollbackBusy, setRollbackBusy] = useState<string | null>(null);
  const status = state.updateStatus;
  const currentVersion = state.bootstrap?.version ?? '1.0.0';
  const updateState = status?.state ?? 'idle';
  const kindKey = status ? updateKindKey(status.kind) : null;
  const activeTransfer =
    status?.state === 'receiving' || status?.state === 'applying';
  const progress = Math.max(0, Math.min(100, status?.progress_percent ?? 0));

  useEffect(() => {
    void refreshUpdateStatus();
  }, [refreshUpdateStatus]);

  const handleChoose = async () => {
    if (choosing || activeTransfer) {
      return;
    }
    setChoosing(true);
    try {
      const selected = await api.chooseUpdatePackage(t('updates.choose'));
      if (selected) {
        setSelectedPath(selected);
      }
    } catch (error) {
      reportError(error, t('updates.pickFailed'));
    } finally {
      setChoosing(false);
    }
  };

  const handlePush = async () => {
    if (pushing || activeTransfer) {
      return;
    }
    if (!selectedPath) {
      notify('warning', t('updates.title'), t('updates.needPackage'));
      return;
    }
    setPushing(true);
    try {
      await api.pushUpdate(
        selectedPath,
        forceInstall,
        packageKind === 'auto' ? null : packageKind,
      );
      notify('success', t('updates.title'), t('updates.pushedToast'));
    } catch (error) {
      reportError(error, t('updates.pushFailed'));
    } finally {
      setPushing(false);
    }
  };

  const handleApply = async () => {
    if (applying || activeTransfer || !status?.version) {
      return;
    }
    setApplying(true);
    try {
      await api.applyUpdate(status.version);
      notify('success', t('updates.apply'));
    } catch (error) {
      reportError(error, t('updates.applyFailed'));
    } finally {
      setApplying(false);
    }
  };

  const handleRollback = async (version: string) => {
    if (rollbackBusy || activeTransfer) {
      return;
    }
    setRollbackBusy(version);
    try {
      await api.rollbackUpdate(version);
    } catch (error) {
      reportError(error, t('updates.rollbackFailed'));
    } finally {
      setRollbackBusy(null);
    }
  };

  return (
    <div className={embedded ? 'updates-embedded' : 'page updates-page'}>
      {!embedded ? (
        <header className="page-head">
          <div className="page-title">
            <div>
              <h1>{t('updates.title')}</h1>
              <div className="chip-row">
                <span className="chip">{t('updates.currentVersion', { version: currentVersion })}</span>
                <span className="chip">{t('updates.lanHint')}</span>
                <span className={`chip status-${updateState}`}>
                  {t(updateStateKey(updateState))}
                </span>
              </div>
            </div>
          </div>
        </header>
      ) : null}

      <div className="updates-grid">
        <section className="card update-package-card">
          <div className="card-head">
            <span>{t('updates.package')}</span>
            <span className="muted">
              {selectedPath ? fileName(selectedPath) : t('updates.noneSelected')}
            </span>
          </div>

          <div className={selectedPath ? 'package-picker has-file' : 'package-picker'}>
            <span className="package-icon" aria-hidden="true">
              <Icon name={selectedPath ? 'file' : 'download'} size={24} />
            </span>
            <div className="package-copy">
              <strong>
                {selectedPath ? fileName(selectedPath) : t('updates.noneSelected')}
              </strong>
              <span title={selectedPath ?? undefined}>
                {selectedPath ?? t('updates.package')}
              </span>
            </div>
            <button
              type="button"
              className="btn"
              disabled={choosing || activeTransfer}
              onClick={() => void handleChoose()}
            >
              {choosing ? (
                <span className="spinner" aria-hidden="true" />
              ) : (
                <Icon name="folder" size={14} />
              )}
              {t('updates.choose')}
            </button>
          </div>

          <div className="update-push-options">
            <SelectMenu
              label={t('updates.packageKind')}
              value={packageKind}
              options={[
                { value: 'auto', label: t('updates.packageKindAuto') },
                { value: 'installer', label: t('updates.kind.installer') },
                { value: 'portable', label: t('updates.kind.portable') },
              ]}
              onChange={(value) =>
                setPackageKind(value as 'auto' | 'installer' | 'portable')
              }
            />
            <div className="setting-row">
              <div className="setting-copy">
                <span className="setting-label">{t('updates.force')}</span>
                <span className="setting-help">{t('updates.forceHelp')}</span>
              </div>
              <Toggle
                checked={forceInstall}
                label={t('updates.force')}
                onChange={setForceInstall}
              />
            </div>
          </div>

          <div className="update-package-footer">
            <span className="muted small">
              <Icon name="shield" size={12} />
              {t('updates.rollbackHint')}
            </span>
            <div className="update-push-actions">
              <button
                type="button"
                className="btn primary"
                disabled={!selectedPath || pushing || activeTransfer}
                onClick={() => void handlePush()}
              >
                {pushing || activeTransfer ? (
                  <span className="spinner" aria-hidden="true" />
                ) : (
                  <Icon name="updates" size={14} />
                )}
                {pushing || activeTransfer ? t('updates.pushing') : t('updates.push')}
              </button>
            </div>
          </div>
        </section>

        <section className="card update-status-card" aria-live="polite">
          <div className="card-head">
            <span>{t('updates.status')}</span>
            <span className={`status-pill status-${updateState}`}>
              {t(updateStateKey(updateState))}
            </span>
          </div>

          <div className="update-meta">
            <div>
              <span>{t('updates.version')}</span>
              <strong>{status?.version ?? t('common.none')}</strong>
            </div>
            <div>
              <span>{t('updates.kind')}</span>
              <strong>{kindKey ? t(kindKey) : t('common.none')}</strong>
            </div>
            <div>
              <span>{t('updates.progress')}</span>
              <strong>{t('updates.percent', { percent: progress })}</strong>
            </div>
          </div>

          <div
            className="progress-track"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={progress}
            aria-label={t('updates.progressAria', { percent: progress })}
          >
            <span className="progress-fill" style={{ width: `${progress}%` }} />
          </div>

          <p className="update-message">
            {status?.message ?? t(updateStateKey(updateState))}
          </p>

          {updateState === 'staged' && status?.version ? (
            <button
              type="button"
              className="btn primary"
              disabled={applying}
              onClick={() => void handleApply()}
            >
              {applying ? (
                <span className="spinner" aria-hidden="true" />
              ) : (
                <Icon name="updates" size={14} />
              )}
              {t('updates.apply')}
            </button>
          ) : null}
        </section>

        <section className="card update-peers-card" aria-live="polite">
          <div className="card-head">
            <span>{t('updates.peers')}</span>
            <span className="muted">
              {t('updates.peerCount', {
                count: status?.peers.length ?? 0,
              })}
            </span>
          </div>
          {!status || status.peers.length === 0 ? (
            <p className="muted empty-inline">{t('updates.noMessage')}</p>
          ) : (
            <ul className="update-peer-list">
              {status.peers.map((peer) => (
                <li key={peer.device_id}>
                  <span className="update-peer-name">
                    <span className={`status-dot tone-${peer.state === 'failed' ? 'danger' : peer.state === 'done' ? 'success' : 'warning'}`} aria-hidden="true" />
                    {peer.device_name}
                  </span>
                  <span className="update-peer-state">
                    {peer.state === 'transferring'
                      ? t('updates.peerProgress', {
                          percent: peer.progress_percent,
                        })
                      : t(peerStateKey(peer.state))}
                  </span>
                  {peer.message ? (
                    <span className="update-peer-message muted small" title={peer.message}>
                      {peer.message}
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="card rollback-card">
          <div className="card-head">
            <span>{t('updates.rollbackTitle')}</span>
            <span className="muted">{status?.rollback_versions.length ?? 0}</span>
          </div>
          {!status || status.rollback_versions.length === 0 ? (
            <p className="muted empty-inline">{t('updates.noRollback')}</p>
          ) : (
            <ul className="rollback-list">
              {status.rollback_versions.map((version) => (
                <li key={version}>
                  <span className="rollback-version">
                    <Icon name="rollback" size={14} />
                    v{version}
                  </span>
                  <button
                    type="button"
                    className="btn compact"
                    disabled={Boolean(rollbackBusy) || activeTransfer}
                    aria-label={t('updates.rollbackTo', { version })}
                    onClick={() => void handleRollback(version)}
                  >
                    {rollbackBusy === version ? (
                      <span className="spinner" aria-hidden="true" />
                    ) : null}
                    {t('updates.rollback')}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </div>
  );
}
