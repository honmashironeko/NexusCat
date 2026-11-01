import { useEffect, useState } from 'react';
import { api, isTauriRuntime } from '../api/client';
import type { EdgeHaloEvent } from '../api/types';
import { applyOverlayTheme } from './overlayTheme';

const ARROWS: Record<string, string> = {
  left: '←',
  right: '→',
  up: '↑',
  down: '↓',
};

export function EdgeHalo() {
  const [payload, setPayload] = useState<EdgeHaloEvent | null>(null);

  useEffect(() => {
    document.documentElement.classList.add('osd-surface');
    document.body.classList.add('osd-surface');
    applyOverlayTheme();
    if (!isTauriRuntime()) {
      setPayload({
        direction: 'right',
        title: 'MacBook Pro',
        display: '内置显示器',
        hint: '停留 100ms 移交',
        available: true,
      });
      return () => {
        document.documentElement.classList.remove('osd-surface');
        document.body.classList.remove('osd-surface');
      };
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void api
      .subscribe('edge-halo', (event) => {
        if (!disposed) {
          setPayload(event);
          applyOverlayTheme();
        }
      })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
        } else {
          unlisten = cleanup;
        }
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
      document.documentElement.classList.remove('osd-surface');
      document.body.classList.remove('osd-surface');
    };
  }, []);

  const direction = payload?.direction ?? 'left';
  const available = payload?.available ?? true;
  return (
    <div
      className={payload ? 'edge-halo is-visible' : 'edge-halo'}
      data-dir={direction}
      data-available={available ? 'true' : 'false'}
      aria-live="polite"
      aria-atomic="true"
    >
      <span className="edge-halo-line" aria-hidden="true" />
      {payload ? (
        <div className="edge-halo-tip">
          <span className="edge-halo-arrow" aria-hidden="true">
            {ARROWS[direction] ?? '•'}
          </span>
          <span className="edge-halo-copy">
            <strong>{payload.title}</strong>
            {payload.display ? <small>{payload.display}</small> : null}
          </span>
          <span className="edge-halo-hint">{payload.hint}</span>
        </div>
      ) : null}
    </div>
  );
}
