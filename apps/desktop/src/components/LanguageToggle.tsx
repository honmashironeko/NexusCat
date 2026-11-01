import { useApp } from '../state/appStore';

export function LanguageToggle({
  compact = false,
  variant = 'segmented',
}: {
  compact?: boolean;
  variant?: 'segmented' | 'button';
}) {
  const { t, language, saveSettings } = useApp();
  if (variant === 'button') {
    const next = language === 'zh' ? 'en' : 'zh';
    const aria = next === 'en' ? t('language.enAria') : t('language.zhAria');
    return (
      <button
        type="button"
        className="language-button"
        aria-label={aria}
        title={aria}
        onClick={() => void saveSettings({ language: next })}
      >
        {next === 'en' ? t('language.en') : t('language.zh')}
      </button>
    );
  }
  const options: Array<{ value: 'zh' | 'en'; label: string; aria: string }> = [
    { value: 'zh', label: t('language.zh'), aria: t('language.zhAria') },
    { value: 'en', label: t('language.en'), aria: t('language.enAria') },
  ];
  return (
    <div
      className={compact ? 'segmented language-toggle compact' : 'segmented language-toggle'}
      role="group"
      aria-label={t('language.label')}
    >
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          className={language === option.value ? 'segment is-active' : 'segment'}
          aria-pressed={language === option.value}
          aria-label={option.aria}
          title={option.aria}
          onClick={() => {
            if (language !== option.value) {
              void saveSettings({ language: option.value });
            }
          }}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
