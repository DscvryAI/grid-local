import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import LanguageDetector from 'i18next-browser-languagedetector';

// Namespace별 JSON import - LLM 친화적 구조
// 각 namespace는 2-40KB로 단일 컨텍스트에서 처리 가능

// English -- the ONLY language eagerly bundled (see `resources` below).
// It's `fallbackLng`, so it must always be synchronously available;
// every other language loads on demand via `lazyLoaders`. Previously all
// 5 languages' ~50 JSON files were imported here unconditionally, forcing
// every user to download and parse ~470kB of translation data for 4
// languages they'd never see.
import enCommon from './locales/en/common.json';
import enAnalytics from './locales/en/analytics.json';
import enSession from './locales/en/session.json';
import enSettings from './locales/en/settings.json';
import enTools from './locales/en/tools.json';
import enError from './locales/en/error.json';
import enMessage from './locales/en/message.json';
import enRenderers from './locales/en/renderers.json';
import enUpdate from './locales/en/update.json';
import enFeedback from './locales/en/feedback.json';

export const supportedLanguages = {
  en: 'English',
  ko: '한국어',
  ja: '日本語',
  'zh-CN': '简体中文',
  'zh-TW': '繁體中文',
} as const;

export type SupportedLanguage = keyof typeof supportedLanguages;

export const languageLocaleMap: Record<string, string> = {
  en: 'en-US',
  ko: 'ko-KR',
  ja: 'ja-JP',
  'zh-CN': 'zh-CN',
  'zh-TW': 'zh-TW',
  'zh-HK': 'zh-HK',
  'zh-MO': 'zh-MO',
};

/**
 * Namespace 목록
 *
 * LLM이 특정 기능의 번역만 수정할 때 해당 namespace 파일만 참조 가능
 */
export const namespaces = [
  'common',
  'analytics',
  'session',
  'settings',
  'tools',
  'error',
  'message',
  'renderers',
  'update',
  'feedback',
] as const;

export type Namespace = (typeof namespaces)[number];

/**
 * Namespace별 리소스 병합 함수
 * i18next는 returnObjects 옵션으로 배열을 반환할 수 있으므로 string | string[] 허용
 */
type TranslationValue = string | string[];
function mergeNamespaces(
  ...nsObjects: Record<string, TranslationValue>[]
): Record<string, TranslationValue> {
  return Object.assign({}, ...nsObjects);
}

// 단일 'translation' namespace로 병합 (기존 호환성 유지)
// 기존 t('prefix.key') 형식이 그대로 동작함
const enResources = mergeNamespaces(
  enCommon,
  enAnalytics,
  enSession,
  enSettings,
  enTools,
  enError,
  enMessage,
  enRenderers,
  enUpdate,
  enFeedback
);

/**
 * One dynamic `import()` per namespace file, per non-English language --
 * each import target matches `vite.config.ts`'s own `manualChunks` rule
 * for that language exactly (`i18n/locales/<lang>/`), so Vite keeps
 * emitting the same per-language chunk files as before; the only change
 * is that a chunk is now actually FETCHED only when its language is
 * loaded, not on every app start regardless of which language is active.
 */
const lazyLoaders: Record<string, () => Promise<Record<string, TranslationValue>>> = {
  ko: async () =>
    mergeNamespaces(
      (await import('./locales/ko/common.json')).default,
      (await import('./locales/ko/analytics.json')).default,
      (await import('./locales/ko/session.json')).default,
      (await import('./locales/ko/settings.json')).default,
      (await import('./locales/ko/tools.json')).default,
      (await import('./locales/ko/error.json')).default,
      (await import('./locales/ko/message.json')).default,
      (await import('./locales/ko/renderers.json')).default,
      (await import('./locales/ko/update.json')).default,
      (await import('./locales/ko/feedback.json')).default
    ),
  ja: async () =>
    mergeNamespaces(
      (await import('./locales/ja/common.json')).default,
      (await import('./locales/ja/analytics.json')).default,
      (await import('./locales/ja/session.json')).default,
      (await import('./locales/ja/settings.json')).default,
      (await import('./locales/ja/tools.json')).default,
      (await import('./locales/ja/error.json')).default,
      (await import('./locales/ja/message.json')).default,
      (await import('./locales/ja/renderers.json')).default,
      (await import('./locales/ja/update.json')).default,
      (await import('./locales/ja/feedback.json')).default
    ),
  'zh-CN': async () =>
    mergeNamespaces(
      (await import('./locales/zh-CN/common.json')).default,
      (await import('./locales/zh-CN/analytics.json')).default,
      (await import('./locales/zh-CN/session.json')).default,
      (await import('./locales/zh-CN/settings.json')).default,
      (await import('./locales/zh-CN/tools.json')).default,
      (await import('./locales/zh-CN/error.json')).default,
      (await import('./locales/zh-CN/message.json')).default,
      (await import('./locales/zh-CN/renderers.json')).default,
      (await import('./locales/zh-CN/update.json')).default,
      (await import('./locales/zh-CN/feedback.json')).default
    ),
  'zh-TW': async () =>
    mergeNamespaces(
      (await import('./locales/zh-TW/common.json')).default,
      (await import('./locales/zh-TW/analytics.json')).default,
      (await import('./locales/zh-TW/session.json')).default,
      (await import('./locales/zh-TW/settings.json')).default,
      (await import('./locales/zh-TW/tools.json')).default,
      (await import('./locales/zh-TW/error.json')).default,
      (await import('./locales/zh-TW/message.json')).default,
      (await import('./locales/zh-TW/renderers.json')).default,
      (await import('./locales/zh-TW/update.json')).default,
      (await import('./locales/zh-TW/feedback.json')).default
    ),
};

/**
 * A minimal, hand-written i18next backend plugin (rather than pulling in
 * `i18next-resources-to-backend` for this one use) -- `read()` is
 * i18next's own documented backend interface, called automatically by
 * `i18n.changeLanguage()`/`i18n.loadLanguages()` whenever a language's
 * resources aren't already loaded. `en` never reaches this (it's in
 * `resources` below, pre-loaded); an unrecognized language code resolves
 * to an empty bundle, which i18next then falls through to `fallbackLng`
 * for -- the same "unsupported language silently falls back to English"
 * behavior the previous all-eager setup had.
 */
const lazyLanguageBackend = {
  type: 'backend' as const,
  init() {},
  read(
    language: string,
    _namespace: string,
    callback: (error: unknown, data: Record<string, TranslationValue> | null) => void
  ) {
    const loader = lazyLoaders[language];
    if (!loader) {
      callback(null, {});
      return;
    }
    loader()
      .then((data) => callback(null, data))
      .catch((error: unknown) => callback(error, null));
  },
};

i18n
  .use(LanguageDetector)
  .use(lazyLanguageBackend)
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: enResources },
    },
    // Lets `en`'s pre-bundled resources and the backend-loaded languages
    // above coexist -- without this, i18next assumes ALL languages must
    // come from the same source (either all `resources`, or all backend)
    // and `en` would incorrectly also be routed through `read()`.
    partialBundledLanguages: true,
    fallbackLng: 'en',
    defaultNS: 'translation',
    ns: ['translation'],

    interpolation: {
      escapeValue: false,
    },

    detection: {
      order: ['localStorage', 'navigator', 'htmlTag'],
      caches: ['localStorage'],
    },

    // The app has no <Suspense> boundary around its root, and
    // react-i18next's default `useSuspense: true` would throw an
    // uncaught Promise (crashing the render) the first time a
    // non-English user's language resources are still loading via
    // `lazyLanguageBackend` above. `false` instead returns the
    // translation KEY synchronously until the real strings arrive, then
    // re-renders -- a brief flash of raw keys on a non-English FIRST
    // load only, never a crash. English (already pre-bundled) is
    // completely unaffected either way.
    react: {
      useSuspense: false,
    },
  });

export default i18n;

// 타입 및 훅 re-export
export { useAppTranslation } from './useAppTranslation';
export type { TranslationKey, TranslationPrefix } from './types.generated';
