import { createContext } from 'react';

export type AppLanguage = 'en' | 'zh';

export type TranslationParams = Record<string, string | number>;

export interface I18nContextValue {
  language: AppLanguage;
  setLanguage: (language: AppLanguage) => void;
  t: (key: string, params?: TranslationParams) => string;
}

export const I18nContext = createContext<I18nContextValue | null>(null);
