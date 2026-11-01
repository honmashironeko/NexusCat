export function applyOverlayTheme(): void {
  let stored = 'system';
  try {
    stored = window.localStorage.getItem('nexuscat.theme') ?? 'system';
  } catch {
  }
  const prefersDark =
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches;
  const theme =
    stored === 'light'
      ? 'light'
      : stored === 'dark'
        ? 'dark'
        : prefersDark
          ? 'dark'
          : 'light';
  document.documentElement.dataset.theme = theme;
}
