import { useEffect, useState } from 'react';
import { api, isTauriRuntime } from '../api/client';
import type { EdgeOsdEvent } from '../api/types';
import { applyOverlayTheme } from './overlayTheme';

const ARROWS: Record<string, string> = {
  left: '←',
  right: '→',
  up: '↑',
  down: '↓',
};

export function EdgeOsd() {
  const [payload, setPayload] = useState<EdgeOsdEvent | null>(null);

  useEffect(() => {
    document.documentElement.classList.add('osd-surface');
    document.body.classList.add('osd-surface');
    applyOverlayTheme();
    if (!isTauriRuntime()) {
      setPayload({
        direction: 'right',
        kicker: '边缘停留 100ms',
        title: 'MacBook Pro',
        display: '内置显示器',
        hint: '键鼠已移交 · 全屏视图',
      });
      return () => {
        document.documentElement.classList.remove('osd-surface');
        document.body.classList.remove('osd-surface');
      };
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void api
      .subscribe('edge-osd', (event) => {
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
  return (
    <div
      className={payload ? 'edge-osd is-visible' : 'edge-osd'}
      data-dir={direction}
      aria-live="polite"
      aria-atomic="true"
    >
      {payload ? (
        <section className="edge-osd-card">
          <div className="edge-osd-top">
            <span className="edge-osd-dir" aria-hidden="true">
              {ARROWS[direction] ?? '•'}
            </span>
            <span className="edge-osd-kicker">{payload.kicker}</span>
          </div>
          <strong className="edge-osd-title">{payload.title}</strong>
          {payload.display ? (
            <span className="edge-osd-display">{payload.display}</span>
          ) : null}
          <span className="edge-osd-hint">{payload.hint}</span>
        </section>
      ) : null}
    </div>
  );
}
