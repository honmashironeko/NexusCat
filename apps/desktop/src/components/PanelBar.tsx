import { api } from '../api/client';
import type { AppView } from '../state/reducer';
import { useApp } from '../state/appStore';
import type { TranslationKey } from '../i18n';
import { Icon, type IconName } from './Icon';
import { LanguageToggle } from './LanguageToggle';
import { Logo } from './Logo';
import { NotificationCenter } from './NotificationCenter';
import { ThemeToggle } from './ThemeToggle';

const PANEL_NAV: Array<{ view: AppView; icon: IconName; labelKey: TranslationKey }> = [
  { view: 'workbench', icon: 'devices', labelKey: 'nav.workbench' },
  { view: 'settings', icon: 'settings', labelKey: 'nav.settings' },
];

export function PanelBar() {
  const { state, t, navigate, notify, reportError } = useApp();

  const runWindowAction = async (action: () => Promise<boolean>) => {
    try {
      const supported = await action();
      if (!supported) {
        notify('info', t('window.mockUnavailable'));
      }
    } catch (error) {
      reportError(error);
    }
  };

  return (
    <header className="panel-bar" data-tauri-drag-region="true">
      <div className="panel-brand" data-tauri-drag-region="true">
        <Logo size={25} />
        <span className="brand-name">NexusCat</span>
        {state.bootstrap?.test_mode ? (
          <span className="test-badge">{t('session.testMode')}</span>
        ) : null}
      </div>

      <nav className="panel-nav" aria-label={t('nav.aria')}>
        {PANEL_NAV.map((entry) => {
          const active = state.view === entry.view;
          return (
            <button
              key={entry.view}
              type="button"
              className={active ? 'nav-item is-active' : 'nav-item'}
              aria-current={active ? 'page' : undefined}
              onClick={() => navigate(entry.view)}
            >
              <Icon name={entry.icon} size={14} />
              <span>{t(entry.labelKey)}</span>
            </button>
          );
        })}
      </nav>

      <div className="panel-tools">
        <NotificationCenter />
        <LanguageToggle variant="button" />
        <ThemeToggle variant="cycle" />
        <span className="panel-tool-divider" aria-hidden="true" />
        <button
          type="button"
          className="icon-button"
          aria-label={t('window.minimize')}
          title={t('window.minimize')}
          onClick={() => void runWindowAction(api.minimizeWindow)}
        >
          <Icon name="minimize" size={14} />
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={t('window.maximize')}
          title={t('window.maximize')}
          onClick={() => void runWindowAction(api.toggleMaximizeWindow)}
        >
          <Icon name="maximize" size={12} />
        </button>
        <button
          type="button"
          className="icon-button window-close"
          aria-label={t('window.closeToTray')}
          title={t('window.closeToTray')}
          onClick={() => void api.hideToBackground().catch((error) => reportError(error))}
        >
          <Icon name="close" size={14} />
        </button>
      </div>
    </header>
  );
}
