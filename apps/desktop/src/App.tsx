import { useEffect, useRef, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api, isTauriRuntime } from './api/client';
import type { ConnectionMode } from './api/types';
import { ControlOsd } from './components/ControlOsd';
import { EdgeHalo } from './components/EdgeHalo';
import { EdgeOsd } from './components/EdgeOsd';
import { FileDropOsd } from './components/FileDropOsd';
import { Icon } from './components/Icon';
import { Logo } from './components/Logo';
import { PairingDialog } from './components/PairingDialog';
import { PanelBar } from './components/PanelBar';
import { PermissionBanner } from './components/PermissionBanner';
import { Toaster } from './components/Toast';
import { useSession } from './hooks/useSession';
import { modeLabelKey } from './domain/labels';
import { Onboarding } from './pages/Onboarding';
import { Settings } from './pages/Settings';
import { Stage } from './pages/Stage';
import { Workbench } from './pages/Workbench';
import { AppProvider, useApp } from './state/appStore';
import type { AppView } from './state/reducer';
import type { TranslationKey } from './i18n';

const PAGE_TITLE_KEYS: Record<AppView, TranslationKey> = {
  workbench: 'nav.workbench',
  settings: 'nav.settings',
};

function LoadingScreen() {
  const { t } = useApp();
  return (
    <div className="boot-screen">
      <Logo size={60} />
      <span className="boot-spinner" aria-hidden="true" />
      <span className="boot-label">{t('common.loading')}</span>
    </div>
  );
}

function ErrorScreen() {
  const { state, t, retry } = useApp();
  return (
    <div className="boot-screen boot-error">
      <Logo size={56} />
      <div className="boot-error-copy">
        <h1>{t('error.loadTitle')}</h1>
        <p>{t('error.loadBody')}</p>
        {state.error ? (
          <details>
            <summary>{t('error.details')}</summary>
            <code>{state.error}</code>
          </details>
        ) : null}
      </div>
      <button type="button" className="btn primary" onClick={retry}>
        <Icon name="refresh" size={14} />
        {t('common.retry')}
      </button>
    </div>
  );
}

function pageForView(view: AppView) {
  return view === 'settings' ? <Settings /> : <Workbench />;
}

function Panel() {
  const { state } = useApp();
  return (
    <div className="panel-scene">
      <div className="panel-shell">
        <PanelBar />
        <PermissionBanner />
        <main
          className={
            state.view === 'workbench'
              ? 'content panel-main is-workbench'
              : 'content panel-main'
          }
        >
          {pageForView(state.view)}
        </main>
      </div>
    </div>
  );
}

function MockTray() {
  const { t, state, notify, reportError } = useApp();
  const { session, peer } = useSession();
  const [busy, setBusy] = useState<ConnectionMode | 'panel' | 'stage' | null>(null);

  const openPanel = async () => {
    setBusy('panel');
    try {
      await api.showControlPanel();
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(null);
    }
  };

  const summonStage = async () => {
    setBusy('stage');
    try {
      await api.summonStage();
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(null);
    }
  };

  const setMode = async (mode: ConnectionMode) => {
    setBusy(mode);
    try {
      await api.setMode(mode);
      if (mode === 'kvm') {
        await api.showControlPanel();
      }
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="mock-tray-shell">
      <section className="mock-tray card" role="dialog" aria-label={t('mockTray.title')}>
        <header className="mock-tray-head">
          <Logo size={32} />
          <div>
            <strong>NexusCat</strong>
            <span>{t('mockTray.subtitle')}</span>
          </div>
          <span className="mock-tray-note">{t('mockTray.note')}</span>
        </header>

        <div className="mock-tray-status">
          <span className="status-dot tone-success" aria-hidden="true" />
          <strong>{peer?.name ?? t('session.noPeer')}</strong>
          <span>{session ? t(modeLabelKey(session.mode)) : t('common.none')}</span>
        </div>

        <div className="mock-tray-mode">
          <span>{t('mockTray.mode')}</span>
          <div className="segmented" role="group" aria-label={t('overview.modeSwitchAria')}>
            {(['kvm', 'aggregation'] as ConnectionMode[]).map((mode) => (
              <button
                key={mode}
                type="button"
                className={session?.mode === mode ? 'segment is-active' : 'segment'}
                aria-pressed={session?.mode === mode}
                disabled={busy !== null}
                onClick={() => void setMode(mode)}
              >
                {t(modeLabelKey(mode))}
              </button>
            ))}
          </div>
        </div>

        <div className="mock-tray-actions">
          <button
            type="button"
            className="btn"
            disabled={busy !== null}
            onClick={() => void openPanel()}
          >
            <Icon name="monitor" size={14} />
            {busy === 'panel' ? t('common.busy') : t('mockTray.openPanel')}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={busy !== null || !peer || session?.mode !== 'aggregation'}
            onClick={() => void summonStage()}
          >
            <Icon name="spark" size={14} />
            {busy === 'stage' ? t('common.busy') : t('mockTray.stage')}
          </button>
        </div>

        <button
          type="button"
          className="mock-tray-quit"
          onClick={() => notify('info', t('mockTray.quitUnavailable'))}
        >
          {t('mockTray.quit')}
        </button>
        <span className="mock-tray-corner">{state.bootstrap?.version ?? '1.0.0'}</span>
      </section>
      <Toaster />
    </div>
  );
}

function AppRoot() {
  const { state, t, isMock } = useApp();
  const { surface } = useSession();
  const panelSizedRef = useRef(false);

  useEffect(() => {
    const title =
      surface === 'stage'
        ? t('stage.title')
        : surface === 'onboarding'
          ? t('onboarding.welcomeTitle')
          : t(PAGE_TITLE_KEYS[state.view]);
    document.title = `NexusCat · ${title}`;
  }, [surface, state.view, t]);

  useEffect(() => {
    if (!isMock && surface === 'onboarding') {
      void api.showControlPanel().catch(() => undefined);
    }
  }, [isMock, surface]);

  useEffect(() => {
    if (isMock || panelSizedRef.current) {
      return;
    }
    if (surface !== 'onboarding' && surface !== 'control') {
      return;
    }
    panelSizedRef.current = true;
    const scaleFactor = window.devicePixelRatio || 1;
    const availableWidth = window.screen.availWidth || 1280;
    const availableHeight = window.screen.availHeight || 720;
    void api
      .setControlPanelSize(true, scaleFactor, availableWidth, availableHeight)
      .catch(() => undefined);
  }, [isMock, surface]);

  const uiState = state.session?.ui_state;
  useEffect(() => {
    if (isMock || (surface !== 'onboarding' && surface !== 'control')) {
      return;
    }
    let cancelled = false;
    let second = 0;
    const first = window.requestAnimationFrame(() => {
      second = window.requestAnimationFrame(() => {
        if (!cancelled) {
          void api.reportSurfaceReady().catch(() => undefined);
        }
      });
    });
    return () => {
      cancelled = true;
      window.cancelAnimationFrame(first);
      window.cancelAnimationFrame(second);
    };
  }, [isMock, surface, uiState]);

  if (surface === 'loading') {
    return <LoadingScreen />;
  }
  if (surface === 'error') {
    return <ErrorScreen />;
  }
  if (surface === 'onboarding') {
    return (
      <>
        <Onboarding />
        <PairingDialog />
        <Toaster />
      </>
    );
  }
  if (surface === 'stage') {
    return <div className="stage-parked" aria-hidden="true" />;
  }
  if (surface === 'background') {
    return (
      <>
        {isMock ? <MockTray /> : null}
        <PairingDialog />
      </>
    );
  }
  return (
    <>
      <Panel />
      <PairingDialog />
      <Toaster />
    </>
  );
}

function StageWindow() {
  const { state, isMock } = useApp();
  const uiState = state.session?.ui_state;
  const active =
    isMock || uiState === 'stage' || state.stageCommand === 'show';
  useEffect(() => {
    if (isMock) {
      return;
    }
    let cancelled = false;
    let second = 0;
    const first = window.requestAnimationFrame(() => {
      second = window.requestAnimationFrame(() => {
        if (!cancelled) {
          void api.reportSurfaceReady().catch(() => undefined);
        }
      });
    });
    return () => {
      cancelled = true;
      window.cancelAnimationFrame(first);
      window.cancelAnimationFrame(second);
    };
  }, [isMock, uiState]);
  return (
    <>
      { }
      <Stage active={active} />
      <PairingDialog />
    </>
  );
}

type OverlaySurface = 'osd' | 'halo' | 'control-osd' | 'file-drop';

function currentWindowLabel(): string | null {
  if (!isTauriRuntime()) {
    return null;
  }
  try {
    return getCurrentWindow().label;
  } catch {
    return null;
  }
}

function overlaySurface(): OverlaySurface | null {
  const requested = new URLSearchParams(window.location.search).get('surface');
  if (
    requested === 'osd' ||
    requested === 'halo' ||
    requested === 'control-osd' ||
    requested === 'file-drop'
  ) {
    return requested;
  }
  const label = currentWindowLabel();
  if (label === 'osd') {
    return 'osd';
  }
  if (label === 'edge-halo') {
    return 'halo';
  }
  if (label === 'control-osd') {
    return 'control-osd';
  }
  if (label === 'file-drop') {
    return 'file-drop';
  }
  return null;
}

export default function App() {
  const surface = overlaySurface();
  if (surface === 'osd') {
    return <EdgeOsd />;
  }
  if (surface === 'halo') {
    return <EdgeHalo />;
  }
  if (surface === 'control-osd') {
    return <ControlOsd />;
  }
  if (surface === 'file-drop') {
    return <FileDropOsd />;
  }
  if (currentWindowLabel() === 'stage') {
    return (
      <AppProvider>
        <StageWindow />
      </AppProvider>
    );
  }
  return (
    <AppProvider>
      <AppRoot />
    </AppProvider>
  );
}
