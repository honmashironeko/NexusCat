import { useEffect, useMemo, useRef, useState } from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { api, isTauriRuntime } from '../api/client';
import type { DisplayNode, FileDropOsdEvent } from '../api/types';
import {
  buildDeviceDropTargets,
  type DeviceDropTarget,
} from '../domain/fileDropDevices';
import { createTranslator, type TranslationKey } from '../i18n';
import { Icon } from './Icon';
import { applyOverlayTheme } from './overlayTheme';

interface PointerPosition {
  x: number;
  y: number;
}

function dragCoordinateScale(): number {
  const isMac =
    typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent);
  return isMac ? 1 : window.devicePixelRatio || 1;
}

function dragPointToCss(position: PointerPosition): PointerPosition {
  const scale = dragCoordinateScale();
  return { x: position.x / scale, y: position.y / scale };
}

const DEMO_NODES: DisplayNode[] = [
  {
    id: 'demo-local:0',
    device_id: 'demo-local',
    device_name: '本机',
    display_name: '内置显示器',
    monitor_index: 0,
    resolution: { width: 1920, height: 1080 },
    dpi: 96,
    physical: { width_mm: 0, height_mm: 0 },
    origin: { x: 0, y: 0 },
    is_local: true,
    is_primary: true,
  },
  {
    id: 'demo-remote:0',
    device_id: 'demo-remote',
    device_name: 'MacBook Pro',
    display_name: '内置显示器',
    monitor_index: 0,
    resolution: { width: 1920, height: 1080 },
    dpi: 96,
    physical: { width_mm: 0, height_mm: 0 },
    origin: { x: 1944, y: 0 },
    is_local: false,
    is_primary: true,
  },
  {
    id: 'demo-remote:1',
    device_id: 'demo-remote',
    device_name: 'MacBook Pro',
    display_name: 'DELL U2720Q',
    monitor_index: 1,
    resolution: { width: 2560, height: 1440 },
    dpi: 96,
    physical: { width_mm: 0, height_mm: 0 },
    origin: { x: 3864, y: 0 },
    is_local: false,
    is_primary: false,
  },
];

export function FileDropOsd() {
  const [payload, setPayload] = useState<FileDropOsdEvent | null>(null);
  const [hoveredDeviceId, setHoveredDeviceId] = useState<string | null>(null);
  const [enteredCount, setEnteredCount] = useState<number | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const payloadRef = useRef<FileDropOsdEvent | null>(null);
  const targetsRef = useRef<DeviceDropTarget[]>([]);
  const reportedHoverRef = useRef<string | null>(null);
  const devicesRef = useRef<HTMLDivElement | null>(null);
  const noticeTimerRef = useRef<number | null>(null);
  const t = useMemo(
    () => createTranslator(payload?.language ?? 'zh'),
    [payload?.language],
  );

  const targets = useMemo(
    () => buildDeviceDropTargets(payload?.nodes ?? []),
    [payload?.nodes],
  );
  targetsRef.current = targets;

  const fileCount = enteredCount ?? payload?.file_count ?? null;
  const hoveredTarget =
    targets.find((target) => target.deviceId === hoveredDeviceId) ?? null;

  useEffect(() => {
    payloadRef.current = payload;
  }, [payload]);

  useEffect(() => {
    if (!isTauriRuntime()) {
      return;
    }
    const report = () => {
      const container = devicesRef.current;
      if (!container) {
        return;
      }
      const scale = dragCoordinateScale();
      const cards = Array.from(
        container.querySelectorAll<HTMLElement>('[data-device-id]'),
      ).flatMap((element) => {
        const deviceId = element.dataset.deviceId;
        const target = targetsRef.current.find(
          (candidate) => candidate.deviceId === deviceId,
        );
        if (!target) {
          return [];
        }
        const rect = element.getBoundingClientRect();
        return [
          {
            node_id: target.nodeId,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
          },
        ];
      });
      void api.setFileDropCards(scale, cards).catch(() => undefined);
    };
    report();
    const observer =
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(() => report());
    if (devicesRef.current) {
      observer?.observe(devicesRef.current);
    }
    window.addEventListener('resize', report);
    return () => {
      observer?.disconnect();
      window.removeEventListener('resize', report);
    };
  }, [payload]);

  useEffect(() => {
    document.documentElement.classList.add('osd-surface');
    document.body.classList.add('osd-surface');
    applyOverlayTheme();
    if (!isTauriRuntime()) {
      const demo: FileDropOsdEvent = {
        nodes: DEMO_NODES,
        file_count: 2,
        enabled: true,
        language: 'zh',
      };
      setPayload(demo);
      return () => {
        document.documentElement.classList.remove('osd-surface');
        document.body.classList.remove('osd-surface');
      };
    }

    let disposed = false;
    let unlistenBackend: (() => void) | undefined;
    let unlistenHidden: (() => void) | undefined;
    let unlistenHover: (() => void) | undefined;
    let unlistenDrag: (() => void) | undefined;

    void api
      .subscribe('file-drop-osd', (event) => {
        if (disposed) {
          return;
        }
        applyOverlayTheme();
        if (noticeTimerRef.current !== null) {
          window.clearTimeout(noticeTimerRef.current);
          noticeTimerRef.current = null;
        }
        setNotice(null);
        setHoveredDeviceId(null);
        setEnteredCount(null);
        reportedHoverRef.current = null;
        setPayload(event);
      })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
        } else {
          unlistenBackend = cleanup;
        }
      })
      .catch(() => undefined);

    void api
      .subscribe('file-drop-osd-hidden', () => {
        if (disposed) {
          return;
        }
        payloadRef.current = null;
        reportedHoverRef.current = null;
        setPayload(null);
        setHoveredDeviceId(null);
        setEnteredCount(null);
        setNotice(null);
      })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
        } else {
          unlistenHidden = cleanup;
        }
      })
      .catch(() => undefined);

    void api
      .subscribe('file-drop-hover', (event) => {
        if (disposed) {
          return;
        }
        const target = event.node_id
          ? (targetsRef.current.find(
              (candidate) => candidate.nodeId === event.node_id,
            ) ?? null)
          : null;
        setHoveredDeviceId(target?.deviceId ?? null);
        if (event.file_count !== null) {
          setEnteredCount(event.file_count > 0 ? event.file_count : null);
        }
      })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
        } else {
          unlistenHover = cleanup;
        }
      })
      .catch(() => undefined);

    const translateNow = (key: TranslationKey) =>
      createTranslator(payloadRef.current?.language ?? 'zh')(key);

    const showNotice = (message: string) => {
      setNotice(message);
      if (noticeTimerRef.current !== null) {
        window.clearTimeout(noticeTimerRef.current);
      }
      noticeTimerRef.current = window.setTimeout(() => {
        noticeTimerRef.current = null;
        setNotice(null);
        void api.hideFileDropOsd().catch(() => undefined);
      }, 1_100);
    };

    const reportHover = (nodeId: string | null) => {
      if (reportedHoverRef.current === nodeId) {
        return;
      }
      reportedHoverRef.current = nodeId;
      void api.setFileDropHover(nodeId).catch(() => undefined);
    };

    const deviceAt = (position: PointerPosition): DeviceDropTarget | null => {
      const point = dragPointToCss(position);
      const element = document.elementFromPoint(point.x, point.y);
      const card = element?.closest<HTMLElement>('[data-device-id]');
      const deviceId = card?.dataset.deviceId;
      if (!deviceId) {
        return null;
      }
      return (
        targetsRef.current.find((target) => target.deviceId === deviceId) ??
        null
      );
    };

    void getCurrentWebview()
      .onDragDropEvent(({ payload: drag }) => {
        if (disposed) {
          return;
        }
        if (drag.type === 'enter') {
          const target = deviceAt(drag.position);
          setEnteredCount(drag.paths.length);
          setHoveredDeviceId(target?.deviceId ?? null);
          reportHover(target?.nodeId ?? null);
          return;
        }
        if (drag.type === 'over') {
          const target = deviceAt(drag.position);
          setHoveredDeviceId(target?.deviceId ?? null);
          reportHover(target?.nodeId ?? null);
          return;
        }
        if (drag.type === 'drop') {
          const current = payloadRef.current;
          const target = deviceAt(drag.position);
          if (!current) {
            void api.hideFileDropOsd().catch(() => undefined);
            return;
          }
          if (!current.enabled) {
            showNotice(translateNow('files.dropDisabled'));
            return;
          }
          if (!target) {
            showNotice(translateNow('files.dropNoTarget'));
            return;
          }
          if (target.isLocal) {
            showNotice(translateNow('files.dropLocal'));
            return;
          }
          return;
        }
        setHoveredDeviceId(null);
        reportHover(null);
      })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
        } else {
          unlistenDrag = cleanup;
        }
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlistenBackend?.();
      unlistenHidden?.();
      unlistenHover?.();
      unlistenDrag?.();
      if (noticeTimerRef.current !== null) {
        window.clearTimeout(noticeTimerRef.current);
      }
      document.documentElement.classList.remove('osd-surface');
      document.body.classList.remove('osd-surface');
    };
  }, []);

  let hint = t('files.dropHint');
  if (payload && !payload.enabled) {
    hint = t('files.dropDisabled');
  } else if (hoveredTarget) {
    hint = hoveredTarget.isLocal
      ? t('files.dropLocal')
      : t('files.dropTarget', { name: hoveredTarget.deviceName });
  } else if (payload && !targets.some((target) => !target.isLocal)) {
    hint = t('files.dropNoRemote');
  }

  return (
    <div
      className={payload ? 'file-drop is-visible' : 'file-drop'}
      role="dialog"
      aria-label={t('files.dropTitle')}
      aria-live="polite"
      aria-atomic="true"
    >
      {payload ? (
        <section className="file-drop-card">
          <header className="file-drop-head">
            <span className="file-drop-icon" aria-hidden="true">
              <Icon name="devices" size={14} />
            </span>
            <strong className="file-drop-title">{t('files.dropTitle')}</strong>
            {fileCount === null ? null : (
              <span className="file-drop-count">
                {t('files.dropCount', { count: fileCount })}
              </span>
            )}
          </header>

          <div className="file-drop-devices" role="list" ref={devicesRef}>
            {targets.map((target) => (
              <div
                key={target.deviceId}
                className={
                  hoveredDeviceId === target.deviceId
                    ? 'file-drop-device is-hover'
                    : 'file-drop-device'
                }
                data-device-id={target.deviceId}
                data-local={target.isLocal ? 'true' : 'false'}
                role="listitem"
                aria-label={
                  target.isLocal
                    ? t('files.dropLocal')
                    : t('files.dropTarget', { name: target.deviceName })
                }
              >
                <span className="file-drop-device-icon" aria-hidden="true">
                  <Icon name="monitor" size={14} />
                </span>
                <span className="file-drop-device-copy">
                  <span className="file-drop-device-name">
                    {target.deviceName}
                    {target.isLocal ? (
                      <span className="file-drop-device-tag">
                        {t('common.local')}
                      </span>
                    ) : null}
                  </span>
                  {target.displays.length > 0 ? (
                    <span className="file-drop-device-displays">
                      {target.displays.join(' · ')}
                    </span>
                  ) : null}
                </span>
                {target.screenCount > 1 ? (
                  <span className="file-drop-device-count">
                    {t('files.dropScreenCount', { count: target.screenCount })}
                  </span>
                ) : null}
              </div>
            ))}
          </div>

          <footer
            className={
              notice
                ? 'file-drop-hint is-notice'
                : hoveredTarget && !hoveredTarget.isLocal && payload.enabled
                  ? 'file-drop-hint is-target'
                  : 'file-drop-hint'
            }
          >
            {notice ?? hint}
          </footer>
        </section>
      ) : null}
    </div>
  );
}
