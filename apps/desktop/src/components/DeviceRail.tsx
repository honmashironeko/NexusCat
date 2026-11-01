import type { DeviceRailEntry } from '../domain/workbench';
import { deviceActionKind } from '../domain/workbench';
import { deviceColorClass } from '../domain/deviceColor';
import { directionLabelKey } from '../domain/labels';
import type { TranslationKey } from '../i18n';
import { useApp } from '../state/appStore';
import { Icon } from './Icon';

const ACTION_LABELS: Record<Exclude<ReturnType<typeof deviceActionKind>, 'none'>, TranslationKey> = {
  pair: 'common.pair',
  connect: 'common.connect',
  control: 'common.control',
  disconnect: 'devices.endControl',
  takeover: 'devices.takeover',
};

export function DeviceRail({
  entries,
  colorByDevice,
  selectedId,
  busyId,
  probing,
  onSelect,
  onAction,
  onProbe,
}: {
  entries: DeviceRailEntry[];
  colorByDevice: Map<string, number>;
  selectedId: string | null;
  busyId: string | null;
  probing: boolean;
  onSelect: (entry: DeviceRailEntry) => void;
  onAction: (entry: DeviceRailEntry) => void;
  onProbe: () => void;
}) {
  const { t } = useApp();

  return (
    <aside className="workbench-rail" aria-label={t('workbench.devices')}>
      <header className="rail-head">
        <span>{t('workbench.devices')}</span>
        <span className="rail-count">{entries.length}</span>
      </header>
      <ul className="rail-list">
        {entries.map((entry) => {
          const action = deviceActionKind(entry);
          const selected = entry.id === selectedId;
          const statusText = entry.isLocal
            ? t('common.local')
            : entry.currentControl
              ? t('devices.currentControl')
              : entry.online
                ? entry.connected
                  ? t('devices.connected')
                  : t('common.online')
                : t('common.offline');
          const tag = entry.isHost
            ? t('devices.host')
            : entry.isTakeoverTarget
              ? t('devices.takeovering')
              : entry.isPending
                ? t('devices.connectingTag')
                : entry.currentControl
                  ? t('devices.currentControl')
                  : entry.isSessionPeer
                    ? t('devices.currentTarget')
                    : null;
          return (
            <li
              key={entry.id}
              className={[
                'rail-item',
                selected ? 'is-selected' : '',
                entry.currentControl ? 'is-current' : '',
                entry.online ? '' : 'is-offline',
              ]
                .filter(Boolean)
                .join(' ')}
            >
              <button
                type="button"
                className="rail-main"
                aria-pressed={selected}
                onClick={() => onSelect(entry)}
              >
                <span
                  className={`rail-icon ${deviceColorClass(colorByDevice.get(entry.id))}`}
                  aria-hidden="true"
                >
                  <Icon
                    name={entry.platform === 'mac_os' ? 'laptop' : 'monitor'}
                    size={15}
                  />
                </span>
                <span className="rail-copy">
                  <span className="rail-name">
                    {entry.name}
                    {tag ? <span className="rail-tag">{tag}</span> : null}
                  </span>
                  <span className="rail-meta">
                    <span
                      className={
                        entry.online || entry.isLocal
                          ? 'status-dot tone-success'
                          : 'status-dot tone-muted'
                      }
                      aria-hidden="true"
                    />
                    {statusText}
                    {entry.displayCount > 0 ? (
                      <>
                        <span className="rail-separator" aria-hidden="true">·</span>
                        {t('devices.displays', { count: entry.displayCount })}
                      </>
                    ) : null}
                    {entry.direction ? (
                      <>
                        <span className="rail-separator" aria-hidden="true">·</span>
                        {t(directionLabelKey(entry.direction))}
                      </>
                    ) : null}
                  </span>
                </span>
              </button>
              {action !== 'none' ? (
                <button
                  type="button"
                  className={
                    action === 'disconnect'
                      ? 'btn ghost-danger compact rail-action'
                      : action === 'pair'
                        ? 'btn primary compact rail-action'
                        : action === 'takeover'
                          ? 'btn primary compact rail-action'
                          : 'btn compact rail-action'
                  }
                  aria-label={
                    action === 'disconnect'
                      ? t('devices.endControlAria', { name: entry.name })
                      : action === 'pair'
                        ? t('devices.pairAria', { name: entry.name })
                        : action === 'takeover'
                          ? t('devices.takeoverAria', { name: entry.name })
                          : action === 'control'
                            ? t('devices.controlAria', { name: entry.name })
                            : t('devices.connectAria', { name: entry.name })
                  }
                  disabled={!entry.online || busyId === entry.id}
                  onClick={() => onAction(entry)}
                >
                  {busyId === entry.id ? (
                    <span className="spinner" aria-hidden="true" />
                  ) : null}
                  {t(ACTION_LABELS[action])}
                </button>
              ) : null}
            </li>
          );
        })}
      </ul>
      <footer className="rail-foot">
        <button type="button" className="btn compact" disabled={probing} onClick={onProbe}>
          {probing ? (
            <span className="spinner" aria-hidden="true" />
          ) : (
            <Icon name="discovery" size={13} />
          )}
          {probing ? t('devices.probing') : t('devices.probe')}
        </button>
      </footer>
    </aside>
  );
}
