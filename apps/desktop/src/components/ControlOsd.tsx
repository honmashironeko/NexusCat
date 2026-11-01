import { useEffect, useState } from 'react';
import { api, isTauriRuntime } from '../api/client';
import type { ControlOsdEvent } from '../api/types';
import { applyOverlayTheme } from './overlayTheme';

const DEMO_PAYLOAD: ControlOsdEvent = {
  kicker: 'NEXUSCAT · 单屏聚合',
  title: '控制权移交铁驭',
  subtitle: 'MacBook Pro 正在控制本机的键盘与鼠标',
  hint: '本机键鼠操作后隐藏',
};

export function ControlOsd() {
  const [payload, setPayload] = useState<ControlOsdEvent | null>(null);

  useEffect(() => {
    document.documentElement.classList.add('osd-surface');
    document.body.classList.add('osd-surface');
    applyOverlayTheme();
    if (!isTauriRuntime()) {
      setPayload(DEMO_PAYLOAD);
      return () => {
        document.documentElement.classList.remove('osd-surface');
        document.body.classList.remove('osd-surface');
      };
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void api
      .subscribe('control-osd', (event) => {
        if (!disposed) {
          setPayload(event);
          if (event) {
            applyOverlayTheme();
          }
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

  return (
    <div
      className={payload ? 'control-osd is-visible' : 'control-osd'}
      aria-live="polite"
      aria-atomic="true"
    >
      {payload ? (
        <section className="control-osd-card">
          <span className="control-osd-kicker">{payload.kicker}</span>
          <span className="control-osd-emblem" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <strong className="control-osd-title">{payload.title}</strong>
          <span className="control-osd-subtitle">{payload.subtitle}</span>
          <span className="control-osd-rule" aria-hidden="true" />
          <span className="control-osd-hint">{payload.hint}</span>
        </section>
      ) : null}
    </div>
  );
}
