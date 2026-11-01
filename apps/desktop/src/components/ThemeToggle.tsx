import type { AppTheme } from '../api/types';
import { useApp } from '../state/appStore';
import { Icon, type IconName } from './Icon';

interface ThemeOption {
  value: AppTheme;
  icon: IconName;
  labelKey: 'theme.system' | 'theme.light' | 'theme.dark';
  ariaKey: 'theme.systemAria' | 'theme.lightAria' | 'theme.darkAria';
}

const THEME_OPTIONS: ThemeOption[] = [
  { value: 'system', icon: 'system', labelKey: 'theme.system', ariaKey: 'theme.systemAria' },
  { value: 'light', icon: 'sun', labelKey: 'theme.light', ariaKey: 'theme.lightAria' },
  { value: 'dark', icon: 'moon', labelKey: 'theme.dark', ariaKey: 'theme.darkAria' },
];

export function ThemeToggle({
  variant = 'segmented',
}: {
  variant?: 'segmented' | 'compact' | 'cycle';
}) {
  const { t, state, resolvedTheme, saveSettings } = useApp();
  if (variant === 'cycle') {
    const nextTheme = resolvedTheme === 'dark' ? 'light' : 'dark';
    const label = nextTheme === 'dark' ? t('theme.darkAria') : t('theme.lightAria');
    return (
      <button
        type="button"
        className="icon-button theme-cycle"
        aria-label={label}
        title={label}
        onClick={() => void saveSettings({ theme: nextTheme })}
      >
        <Icon name={resolvedTheme === 'dark' ? 'moon' : 'sun'} size={15} />
      </button>
    );
  }
  const compact = variant === 'compact';
  return (
    <div
      className={compact ? 'segmented theme-toggle compact' : 'segmented theme-toggle'}
      role="group"
      aria-label={t('theme.label')}
    >
      {THEME_OPTIONS.map((option) => {
        const label = t(option.labelKey);
        const aria = t(option.ariaKey);
        const active = state.settings.theme === option.value;
        return (
          <button
            key={option.value}
            type="button"
            className={active ? 'segment is-active' : 'segment'}
            aria-pressed={active}
            aria-label={aria}
            title={label}
            onClick={() => {
              if (!active) {
                void saveSettings({ theme: option.value });
              }
            }}
          >
            <Icon name={option.icon} size={compact ? 14 : 15} />
            {compact ? null : <span>{label}</span>}
          </button>
        );
      })}
    </div>
  );
}
