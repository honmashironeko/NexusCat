import type { AppLanguage } from '../api/types';

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const;

function localeFor(language: AppLanguage): string {
  return language === 'zh' ? 'zh-CN' : 'en-US';
}

function formatNumber(value: number, language: AppLanguage, maximumFractionDigits = 1): string {
  return new Intl.NumberFormat(localeFor(language), {
    maximumFractionDigits,
  }).format(value);
}

export function formatBytes(bytes: number, language: AppLanguage = 'en'): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return '0 B';
  }
  const unitIndex = Math.min(
    BYTE_UNITS.length - 1,
    Math.floor(Math.log(bytes) / Math.log(1024)),
  );
  const value = bytes / 1024 ** unitIndex;
  const digits = unitIndex === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  const localized = new Intl.NumberFormat(localeFor(language), {
    maximumFractionDigits: digits,
  }).format(value);
  return `${localized} ${BYTE_UNITS[unitIndex]}`;
}

export function formatBitrate(kbps: number, language: AppLanguage = 'en'): string {
  if (!Number.isFinite(kbps) || kbps <= 0) {
    return '0 kbps';
  }
  if (kbps >= 1000) {
    return `${formatNumber(kbps / 1000, language, 1)} Mbps`;
  }
  return `${formatNumber(kbps, language, 0)} kbps`;
}

export function formatMilliseconds(milliseconds: number, language: AppLanguage = 'en'): string {
  const value = Number.isFinite(milliseconds) ? Math.max(0, milliseconds) : 0;
  return `${formatNumber(value, language, value >= 10 ? 0 : 1)} ms`;
}

export function formatFps(fps: number, language: AppLanguage = 'en'): string {
  const value = Number.isFinite(fps) ? Math.max(0, fps) : 0;
  return `${formatNumber(value, language, 0)} fps`;
}

export function formatPercent(percent: number, language: AppLanguage = 'en'): string {
  const value = Number.isFinite(percent) ? Math.max(0, percent) : 0;
  const fractionDigits = value < 0.1 ? 2 : value < 10 ? 1 : 0;
  return `${formatNumber(value, language, fractionDigits)}%`;
}

export function formatRelativeTime(
  timestampMs: number,
  language: AppLanguage = 'en',
  nowMs: number = Date.now(),
): string {
  if (!Number.isFinite(timestampMs) || timestampMs <= 0) {
    return '—';
  }
  const elapsed = Math.max(0, nowMs - timestampMs);
  const locale = localeFor(language);
  if (elapsed < 30_000) {
    return new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(0, 'second');
  }
  if (elapsed < 60 * 60_000) {
    return new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(
      -Math.floor(elapsed / 60_000),
      'minute',
    );
  }
  if (elapsed < 24 * 60 * 60_000) {
    return new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(
      -Math.floor(elapsed / (60 * 60_000)),
      'hour',
    );
  }
  if (elapsed < 30 * 24 * 60 * 60_000) {
    return new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(
      -Math.floor(elapsed / (24 * 60 * 60_000)),
      'day',
    );
  }
  return new Intl.DateTimeFormat(locale, {
    month: 'short',
    day: 'numeric',
  }).format(new Date(timestampMs));
}

export function formatClock(timestampMs: number, language: AppLanguage = 'en'): string {
  return new Intl.DateTimeFormat(localeFor(language), {
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(timestampMs));
}

export function formatFileCount(count: number, language: AppLanguage = 'en'): string {
  return new Intl.NumberFormat(localeFor(language)).format(count);
}
