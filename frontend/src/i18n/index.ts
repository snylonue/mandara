// Frontend i18n setup (i18next).
//
// Only zh-CN is shipped for now; the design supports more locales:
//  1. add `src/i18n/locales/<tag>.json` with the same keys
//  2. register it in `resources` below
//  3. (optional) wire a detector to persist the user's choice

import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import zhCN from "./locales/zh-CN.json";

export const SUPPORTED_LOCALES = ["zh-CN"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];

void i18n.use(initReactI18next).init({
  resources: {
    "zh-CN": { translation: zhCN },
  },
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  interpolation: {
    // React already escapes; JSON values never contain raw HTML.
    escapeValue: false,
  },
});

export default i18n;