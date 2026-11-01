
import { useEffect, useMemo, useState } from 'react';
import { api } from '../api/client';
import type { ConnectionMode } from '../api/types';
import { DisplayInspector } from '../components/DisplayInspector';
import { DeviceRail } from '../components/DeviceRail';
import { Icon } from '../components/Icon';
import { LayoutCanvas } from '../components/LayoutCanvas';
import { Modal } from '../components/Modal';
import { TransferList } from '../components/TransferList';
import { buildDeviceGroups } from '../domain/deviceColor';
import { localDeviceId } from '../domain/group';
import { modeLabelKey, sessionStateKey } from '../domain/labels';
import { buildDeviceRailEntries, deviceActionKind, type DeviceRailEntry } from '../domain/workbench';
import { useLayoutDraft } from '../hooks/useLayoutDraft';
import { useSession } from '../hooks/useSession';
import { useApp } from '../state/appStore';

const CONNECTED_STATES = ['connected', 'kvm', 'aggregating', 'reconnecting'];

export function Workbench() {
  const { state, t, notify, reportError } = useApp();
  const { session } = useSession();
  const draft = useLayoutDraft();
  const [selectedDeviceId, setSelectedDeviceId] = useState<string | null>(null);
  const [busyDeviceId, setBusyDeviceId] = useState<string | null>(null);
  const [probing, setProbing] = useState(false);
  const [modeBusy, setModeBusy] = useState(false);
  const [sendingFiles, setSendingFiles] = useState(false);
  const [transferOpen, setTransferOpen] = useState(false);
  const [cancellingTransfer, setCancellingTransfer] = useState<number | null>(null);
  const [forgetTarget, setForgetTarget] = useState<DeviceRailEntry | null>(null);
  const [forgetBusy, setForgetBusy] = useState(false);
  const [fullscreenBusy, setFullscreenBusy] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);

  const localId = localDeviceId(state.layout);
  const entries = useMemo(
    () =>
      buildDeviceRailEntries(
        state.devices,
        state.layout,
        state.session,
        {
          id: localId,
          name: state.settings.device_name || t('common.local'),
          platform: state.bootstrap?.platform ?? 'windows',
        },
        (index) => t('layout.monitorIndex', { index }),
        state.groupStatus,
      ),
    [
      state.devices,
      state.layout,
      state.session,
      state.groupStatus,
      localId,
      state.settings.device_name,
      state.bootstrap?.platform,
      t,
    ],
  );
  const colorByDevice = useMemo(
    () => new Map(buildDeviceGroups(draft.visibleNodes).map((group) => [group.deviceId, group.colorIndex])),
    [draft.visibleNodes],
  );

  const selectedEntry =
    entries.find((entry) => entry.id === selectedDeviceId) ??
    (draft.selectedNode
      ? entries.find((entry) => entry.id === draft.selectedNode?.device_id)
      : undefined) ??
    entries.find((entry) => entry.currentControl) ??
    entries[0] ??
    null;

  useEffect(() => {
    if (
      selectedEntry?.primaryNodeId &&
      draft.selectedNode?.device_id !== selectedEntry.id
    ) {
      draft.setSelectedId(selectedEntry.primaryNodeId);
    }
  }, [
    selectedEntry?.id,
    selectedEntry?.primaryNodeId,
    draft.selectedNode?.device_id,
    draft.setSelectedId,
  ]);

  const currentPeer = session?.peer;
  const sessionConnected = Boolean(
    currentPeer && session && CONNECTED_STATES.includes(session.state),
  );
  const hostEntry = entries.find((entry) => entry.isHost) ?? null;
  const pendingEntry = entries.find((entry) => entry.isPending) ?? null;
  const takeoverEntry = entries.find((entry) => entry.isTakeoverTarget) ?? null;
  const fileTransferEnabled = state.settings.file_transfer_enabled;
  const mode = session?.mode ?? state.settings.mode;
  const syncActive = state.sharedConfig.state === 'synced';
  const syncBusy = state.sharedConfig.state === 'syncing';

  const selectEntry = (entry: DeviceRailEntry) => {
    setSelectedDeviceId(entry.id);
    if (entry.primaryNodeId) {
      draft.setSelectedId(entry.primaryNodeId);
    }
  };

  const selectNode = (nodeId: string) => {
    draft.setSelectedId(nodeId);
    const node = state.layout.nodes.find((item) => item.id === nodeId);
    if (node) {
      setSelectedDeviceId(node.device_id);
    }
  };

  const runAction = async (entry: DeviceRailEntry) => {
    if (busyDeviceId) {
      return;
    }
    const action = deviceActionKind(entry);
    if (action === 'none') {
      return;
    }
    setBusyDeviceId(entry.id);
    try {
      if (action === 'pair') {
        await api.pairDevice(entry.id);
      } else if (action === 'disconnect') {
        await api.disconnect();
      } else {
        await api.connectDevice(entry.id);
      }
    } catch (error) {
      reportError(error, t('devices.actionFailed'));
    } finally {
      setBusyDeviceId(null);
    }
  };

  const retryConnection = async () => {
    if (retrying) {
      return;
    }
    setRetrying(true);
    try {
      await api.retryConnection();
    } catch (error) {
      reportError(error);
    } finally {
      setRetrying(false);
    }
  };

  const probeDevices = async () => {
    if (probing) {
      return;
    }
    setProbing(true);
    try {
      await api.probeDevices();
    } catch (error) {
      reportError(error, t('devices.probeFailed'));
    } finally {
      setProbing(false);
    }
  };

  const setMode = async (next: ConnectionMode) => {
    if (modeBusy || mode === next) {
      return;
    }
    setModeBusy(true);
    try {
      await api.setMode(next);
    } catch (error) {
      reportError(error);
    } finally {
      setModeBusy(false);
    }
  };

  const summonStage = async () => {
    if (fullscreenBusy) {
      return;
    }
    setFullscreenBusy(true);
    try {
      await api.summonStage();
    } catch (error) {
      reportError(error, t('toast.noStage'));
    } finally {
      setFullscreenBusy(false);
    }
  };

  const sendFiles = async () => {
    if (sendingFiles || !sessionConnected || !fileTransferEnabled) {
      return;
    }
    setSendingFiles(true);
    try {
      const files = await api.chooseFiles(t('files.choose'));
      if (files.length > 0) {
        await api.sendFiles(files);
        notify('success', t('files.title'), t('files.sentToast'));
      }
    } catch (error) {
      reportError(error, t('files.sendFailed'));
    } finally {
      setSendingFiles(false);
    }
  };

  const cancelTransfer = async (id: number) => {
    if (cancellingTransfer !== null) {
      return;
    }
    setCancellingTransfer(id);
    try {
      await api.cancelTransfer(id);
    } catch (error) {
      reportError(error, t('files.sendFailed'));
    } finally {
      setCancellingTransfer(null);
    }
  };

  const confirmForget = async () => {
    if (!forgetTarget || forgetBusy) {
      return;
    }
    setForgetBusy(true);
    try {
      await api.removeTrustedDevice(forgetTarget.id);
      notify(
        'success',
        t('devices.removeTrustTitle'),
        t('devices.removeTrustSuccess', { name: forgetTarget.name }),
      );
      setForgetTarget(null);
    } catch (error) {
      reportError(error, t('devices.removeTrustFailed'));
    } finally {
      setForgetBusy(false);
    }
  };

  const endControl = async () => {
    if (disconnecting) {
      return;
    }
    setDisconnecting(true);
    try {
      await api.disconnect();
    } catch (error) {
      reportError(error, t('devices.actionFailed'));
    } finally {
      setDisconnecting(false);
    }
  };

  const syncNow = async () => {
    try {
      await api.syncSharedConfig();
    } catch (error) {
      reportError(error);
    }
  };

  const finishedTransfers = state.transfers.some(
    (transfer) =>
      transfer.state === 'done' ||
      transfer.state === 'failed' ||
      transfer.state === 'cancelled',
  );

  const sessionDot =
    sessionConnected
      ? 'status-dot tone-success'
      : session?.state === 'error'
        ? 'status-dot tone-danger'
        : 'status-dot tone-muted';

  return (
    <div className="page workbench-page">
      <div className="workbench-grid">
        <DeviceRail
          entries={entries}
          colorByDevice={colorByDevice}
          selectedId={selectedEntry?.id ?? null}
          busyId={busyDeviceId}
          probing={probing}
          onSelect={selectEntry}
          onAction={(entry) => void runAction(entry)}
          onProbe={() => void probeDevices()}
        />

        <section className="workbench-stage" aria-label={t('layout.canvasAria')}>
          <header className="workbench-stage-head">
            <div className="chip-row">
              <span className="chip">
                {t('layout.nodeCount', { count: draft.visibleNodes.length })}
              </span>
              {draft.saving ? (
                <span className="chip chip-warning">{t('layout.autoSaving')}</span>
              ) : draft.dirty ? (
                <span className="chip chip-warning">{t('layout.dirty')}</span>
              ) : (
                <span className="chip chip-success">{t('layout.autoSaved')}</span>
              )}
              <span className="workbench-hint">{t('workbench.canvasHint')}</span>
            </div>
            <div className="head-actions">
              <button
                type="button"
                className="btn compact"
                disabled={draft.historyCount === 0 || draft.saving}
                onClick={draft.undo}
              >
                <Icon name="rollback" size={13} />
                {t('layout.undo')}
              </button>
              <button type="button" className="btn compact" onClick={draft.requestFit}>
                <Icon name="maximize" size={13} />
                {t('layout.fitCanvas')}
              </button>
            </div>
          </header>
          <LayoutCanvas
            nodes={draft.visibleNodes}
            selectedId={draft.selectedId}
            currentNodeId={session?.current_display_node ?? null}
            revision={state.layout.revision}
            fitRequest={draft.fitRequest}
            deviceColorByDevice={draft.deviceColorByDevice}
            onSelect={selectNode}
            onNodeMove={draft.handleMove}
            onDragStart={draft.handleDragStart}
            onDragEnd={draft.handleDragEnd}
          />
        </section>

        <DisplayInspector
          entry={selectedEntry}
          draft={draft}
          busy={busyDeviceId === selectedEntry?.id}
          retrying={retrying}
          canRetry={Boolean(
            selectedEntry &&
              (selectedEntry.isPending ||
                selectedEntry.isTakeoverTarget ||
                (selectedEntry.isSessionPeer && session?.stage_visible)),
          )}
          onAction={(entry) => void runAction(entry)}
          onRetry={() => void retryConnection()}
          onForget={setForgetTarget}
        />
      </div>

      <footer className="workbench-actions">
        <div className="workbench-status" role="status">
          <span className={sessionDot} aria-hidden="true" />
          <span>{t(sessionStateKey(session?.state ?? 'idle'))}</span>
          <span className="rail-separator" aria-hidden="true">·</span>
          <span>
            {t('session.input.label')}{' '}
            {session?.input_owner === 'local'
              ? t('session.input.local')
              : t('session.input.remote')}
          </span>
          <span className="rail-separator" aria-hidden="true">·</span>
          <span className={hostEntry?.isLocal ? 'host-pill is-local' : 'host-pill'}>
            {hostEntry
              ? t('workbench.hostValue', { name: hostEntry.name })
              : t('workbench.noHost')}
          </span>
          {takeoverEntry ? (
            <>
              <span className="rail-separator" aria-hidden="true">·</span>
              <span className="host-pill is-pending">
                {t('devices.takeoveringValue', { name: takeoverEntry.name })}
              </span>
            </>
          ) : pendingEntry ? (
            <>
              <span className="rail-separator" aria-hidden="true">·</span>
              <span className="host-pill is-pending">
                {t('devices.connectingValue', { name: pendingEntry.name })}
              </span>
            </>
          ) : null}
          <span className="rail-separator" aria-hidden="true">·</span>
          <span className={syncActive ? 'sync-pill is-synced' : 'sync-pill'}>
            {syncActive
              ? t('overview.synced')
              : syncBusy
                ? t('overview.syncing')
                : t('overview.localConfig')}
          </span>
          <button
            type="button"
            className="icon-button"
            title={t('settings.syncNow')}
            aria-label={t('settings.syncNow')}
            disabled={state.sharedConfig.online_devices === 0}
            onClick={() => void syncNow()}
          >
            <Icon name="refresh" size={13} />
          </button>
        </div>

        <div className="segmented mode-switch" role="group" aria-label={t('overview.modeSwitchAria')}>
          {(['kvm', 'aggregation'] as ConnectionMode[]).map((option) => (
            <button
              key={option}
              type="button"
              className={mode === option ? 'segment is-active' : 'segment'}
              aria-pressed={mode === option}
              disabled={modeBusy}
              onClick={() => void setMode(option)}
            >
              {t(modeLabelKey(option))}
            </button>
          ))}
        </div>

        <div className="workbench-buttons">
          {sessionConnected ? (
            <button
              type="button"
              className="btn ghost-danger compact"
              disabled={disconnecting}
              onClick={() => void endControl()}
            >
              {disconnecting ? (
                <span className="spinner" aria-hidden="true" />
              ) : (
                <Icon name="stop" size={13} />
              )}
              {t('devices.endControl')}
            </button>
          ) : null}
          <button type="button" className="btn compact" onClick={() => setTransferOpen(true)}>
            <Icon name="file" size={13} />
            {t('files.title')}
            {state.transfers.length > 0 ? (
              <span className="rail-count">{state.transfers.length}</span>
            ) : null}
          </button>
          <button
            type="button"
            className="btn compact"
            disabled={!sessionConnected || !fileTransferEnabled || sendingFiles}
            onClick={() => void sendFiles()}
          >
            {sendingFiles ? (
              <span className="spinner" aria-hidden="true" />
            ) : (
              <Icon name="file" size={13} />
            )}
            {t('files.send')}
          </button>
          {sessionConnected && mode === 'aggregation' ? (
            <button
              type="button"
              className="btn primary compact"
              disabled={fullscreenBusy}
              onClick={() => void summonStage()}
            >
              <Icon name="spark" size={13} />
              {fullscreenBusy ? t('common.busy') : t('overview.summonStage')}
            </button>
          ) : null}
        </div>
      </footer>

      <Modal
        open={transferOpen}
        title={t('files.title')}
        size="lg"
        onClose={() => setTransferOpen(false)}
        footer={
          <>
            <button
              type="button"
              className="btn ghost compact"
              onClick={() =>
                void api
                  .openDownloadsFolder()
                  .catch((error) => reportError(error))
              }
            >
              {t('files.openFolder')}
            </button>
            {finishedTransfers ? (
              <button
                type="button"
                className="btn compact"
                onClick={() =>
                  void api
                    .clearFinishedTransfers()
                    .catch((error) => reportError(error))
                }
              >
                {t('files.clearFinished')}
              </button>
            ) : null}
            <button
              type="button"
              className="btn primary compact"
              onClick={() => setTransferOpen(false)}
            >
              {t('common.close')}
            </button>
          </>
        }
      >
        <TransferList
          transfers={state.transfers}
          cancellingId={cancellingTransfer}
          onCancel={(id) => void cancelTransfer(id)}
        />
      </Modal>

      <Modal
        open={Boolean(forgetTarget)}
        title={t('devices.removeTrustTitle')}
        size="sm"
        onClose={() => {
          if (!forgetBusy) {
            setForgetTarget(null);
          }
        }}
        footer={
          <>
            <button
              type="button"
              className="btn ghost"
              disabled={forgetBusy}
              onClick={() => setForgetTarget(null)}
            >
              {t('common.cancel')}
            </button>
            <button
              type="button"
              className="btn ghost-danger"
              disabled={forgetBusy}
              onClick={() => void confirmForget()}
            >
              {forgetBusy ? t('common.busy') : t('devices.removeTrustConfirm')}
            </button>
          </>
        }
      >
        <p>
          {t('devices.removeTrustBody', {
            name: forgetTarget?.name ?? t('common.unknown'),
          })}
        </p>
      </Modal>
    </div>
  );
}
