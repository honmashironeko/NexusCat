
import type { DisplayNode } from '../api/types';
import type { LayoutDraft } from '../hooks/useLayoutDraft';
import type { DeviceRailEntry } from '../domain/workbench';
import { deviceActionKind } from '../domain/workbench';
import { displaysForDevice } from '../domain/group';
import { deviceColorClass } from '../domain/deviceColor';
import { directionFromLocal, type LayoutDirection } from '../domain/layout';
import { directionLabelKey, platformLabelKey } from '../domain/labels';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';
import { SelectMenu } from './SelectMenu';

export function DisplayInspector({
  entry,
  draft,
  busy,
  retrying,
  canRetry,
  onAction,
  onRetry,
  onForget,
}: {
  entry: DeviceRailEntry | null;
  draft: LayoutDraft;
  busy: boolean;
  retrying: boolean;
  canRetry: boolean;
  onAction: (entry: DeviceRailEntry) => void;
  onRetry: (entry: DeviceRailEntry) => void;
  onForget: (entry: DeviceRailEntry) => void;
}) {
  const { state, t } = useApp();
  if (!entry) {
    return (
      <aside className="workbench-inspector card inspector-empty">
        <Icon name="devices" size={22} />
        <span>{t('workbench.noSelection')}</span>
      </aside>
    );
  }

  const displays = displaysForDevice(state.layout, entry.id);
  const selected =
    draft.selectedNode && draft.selectedNode.device_id === entry.id
      ? draft.selectedNode
      : displays[0] ?? null;
  const action = deviceActionKind(entry);
  const selectedPortrait = Boolean(
    selected && selected.resolution.height > selected.resolution.width,
  );
  const colorClass = deviceColorClass(draft.deviceColorByDevice.get(entry.id));
  const direction = (node: DisplayNode): LayoutDirection | null =>
    directionFromLocal(draft.visibleNodes, node.id);
  const actionLabel =
    action === 'disconnect'
      ? t('devices.endControl')
      : action === 'pair'
        ? t('common.pair')
        : action === 'takeover'
          ? t('devices.takeover')
          : action === 'control'
            ? t('common.control')
            : t('common.connect');

  return (
    <aside className="workbench-inspector card" aria-label={t('workbench.details')}>
      <header className="inspector-head">
        <div>
          <h2 className={colorClass}>
            <span className="device-dot" aria-hidden="true" />
            {entry.name}
          </h2>
          <span className="inspector-subtitle">
            {t(platformLabelKey(entry.platform))}
            <span className="rail-separator" aria-hidden="true">·</span>
            {entry.isHost
              ? t('devices.host')
              : entry.isLocal
                ? t('common.local')
                : entry.currentControl
                  ? t('devices.currentControl')
                  : entry.isSessionPeer
                    ? t('devices.currentTarget')
                    : entry.isTakeoverTarget
                      ? t('devices.takeovering')
                      : entry.online
                        ? t('common.online')
                        : t('common.offline')}
            {entry.displayCount > 0 ? (
              <>
                <span className="rail-separator" aria-hidden="true">·</span>
                {t('devices.displays', { count: entry.displayCount })}
              </>
            ) : null}
          </span>
        </div>
        {!entry.isLocal ? (
          <span
            className={
              entry.online ? 'status-dot tone-success' : 'status-dot tone-muted'
            }
            aria-hidden="true"
          />
        ) : null}
      </header>

      <section className="inspector-section">
        <div className="inspector-section-head">
          <span>{t('workbench.displays')}</span>
        </div>
        {displays.length === 0 ? (
          <p className="inspector-empty-line">{t('devices.noDisplays')}</p>
        ) : (
          <ul className="display-list">
            {displays.map((node) => {
              const nodeDirection = direction(node);
              const active = selected?.id === node.id;
              return (
                <li key={node.id}>
                  <button
                    type="button"
                    className={active ? 'display-item is-active' : 'display-item'}
                    aria-pressed={active}
                    onClick={() => draft.setSelectedId(node.id)}
                  >
                    <span className="display-item-name">
                      {
                        node.display_name.trim() ||
                        t('layout.monitorIndex', { index: node.monitor_index + 1 })
                      }
                      {node.is_primary ? (
                        <span className="rail-tag">{t('settings.primaryDisplay')}</span>
                      ) : null}
                    </span>
                    <span className="display-item-meta">
                      {node.resolution.width}×{node.resolution.height}
                      {nodeDirection ? ` · ${t(directionLabelKey(nodeDirection))}` : ''}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {selected ? (
        <>
          <section className="inspector-section">
            <div className="inspector-section-head">
              <span>{t('workbench.displaySettings')}</span>
            </div>
            <dl className="inspector-meta">
              <div>
                <dt>{t('layout.displayResolution')}</dt>
                <dd>
                  {selected.resolution.width}×{selected.resolution.height}
                </dd>
              </div>
              <div>
                <dt>{t('layout.monitor')}</dt>
                <dd>#{selected.monitor_index + 1}</dd>
              </div>
              <div>
                <dt>{t('layout.orientation')}</dt>
                <dd className="orientation-switch">
                  <button
                    type="button"
                    className={selectedPortrait ? '' : 'is-active'}
                    onClick={() => draft.setOrientationFor(selected.id, 'landscape')}
                  >
                    {t('layout.landscape')}
                  </button>
                  <button
                    type="button"
                    className={selectedPortrait ? 'is-active' : ''}
                    onClick={() => draft.setOrientationFor(selected.id, 'portrait')}
                  >
                    {t('layout.portrait')}
                  </button>
                </dd>
              </div>
            </dl>
          </section>

          {!entry.isLocal ? (
            <section className="inspector-section">
              <div className="inspector-section-head">
                <span>{t('layout.videoTarget')}</span>
              </div>
              <div className="media-select-row">
                <SelectMenu
                  label={t('layout.outputResolution')}
                  value={draft.resolutionValue}
                  options={draft.resolutionOptions}
                  onChange={draft.selectResolution}
                />
                <SelectMenu
                  label={t('layout.outputFps')}
                  value={draft.fpsValue}
                  options={draft.fpsOptions}
                  onChange={draft.selectFps}
                />
              </div>
            </section>
          ) : null}
        </>
      ) : null}

      {action !== 'none' || (!entry.isLocal && entry.trusted) ? (
        <footer className="inspector-actions">
          {canRetry ? (
            <button
              type="button"
              className="btn compact"
              disabled={busy || retrying}
              onClick={() => onRetry(entry)}
            >
              {retrying ? (
                <span className="spinner" aria-hidden="true" />
              ) : (
                <Icon name="refresh" size={12} />
              )}
              {t('devices.retryConnection')}
            </button>
          ) : null}
          {action !== 'none' ? (
            <button
              type="button"
              className={
                action === 'disconnect'
                  ? 'btn ghost-danger compact'
                  : action === 'pair' || action === 'takeover'
                    ? 'btn primary compact'
                    : 'btn'
              }
              disabled={!entry.online || busy || retrying}
              onClick={() => onAction(entry)}
            >
              {busy ? <span className="spinner" aria-hidden="true" /> : null}
              {actionLabel}
            </button>
          ) : null}
          {!entry.isLocal && entry.trusted ? (
            <button
              type="button"
              className="text-button danger"
              disabled={busy}
              onClick={() => onForget(entry)}
            >
              {t('devices.removeTrustAction')}
            </button>
          ) : null}
        </footer>
      ) : null}
    </aside>
  );
}
