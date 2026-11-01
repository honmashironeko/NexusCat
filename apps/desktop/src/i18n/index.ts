import type { AppLanguage } from '../api/types';
import { en } from './en';
import { zh } from './zh';

export type TranslationKey = keyof typeof zh;
export type TranslationVars = Record<string, string | number>;
export type Translate = (key: TranslationKey, vars?: TranslationVars) => string;

export const dictionaries: Record<AppLanguage, Record<TranslationKey, string>> = {
  zh,
  en,
};

export function localeFor(language: AppLanguage): string {
  return language === 'zh' ? 'zh-CN' : 'en-US';
}

export function translate(
  language: AppLanguage,
  key: TranslationKey,
  vars?: TranslationVars,
): string {
  const template = dictionaries[language][key] ?? dictionaries.zh[key] ?? key;
  if (!vars) {
    return template;
  }
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = vars[name];
    return value === undefined ? match : String(value);
  });
}

export function createTranslator(language: AppLanguage): Translate {
  return (key, vars) => translate(language, key, vars);
}
