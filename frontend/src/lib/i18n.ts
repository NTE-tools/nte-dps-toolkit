import { create } from "zustand";

import simplifiedChinese from "@res/languages/zh-CN.json";

import type { Language } from "@/lib/tauri/generated/Language";

type LocaleDictionary = Record<string, string>;

const dictionaries: Partial<Record<Exclude<Language, "en">, LocaleDictionary>> =
  {
    "zh-CN": simplifiedChinese,
  };
let japaneseLoad: Promise<LocaleDictionary> | null = null;

interface FrontendLanguageState {
  language: Language;
  revision: number;
}

const useFrontendLanguageStore = create<FrontendLanguageState>(() => ({
  language: "zh-CN",
  revision: 0,
}));

/** The frontend language; components that render `t` output subscribe to it. */
export function useFrontendLanguage(): Language {
  return useFrontendLanguageStore((state) => state).language;
}

export function getFrontendLanguage(): Language {
  return useFrontendLanguageStore.getState().language;
}

export function t(key: string): string {
  const language = useFrontendLanguageStore.getState().language;
  return language === "en" ? key : (dictionaries[language]?.[key] ?? key);
}

export function tf(key: string, arguments_: readonly string[]): string {
  return formatTemplate(t(key), arguments_);
}

/**
 * Formats the shared Rust/TypeScript placeholder grammar. `{n}` reuses argument
 * n anywhere in the template; when the current argument has no indexed token,
 * it consumes the next `{}` token. Unknown or unfilled tokens stay literal.
 */
export function formatTemplate(
  template: string,
  arguments_: readonly string[],
): string {
  return arguments_.reduce((message, argument, index) => {
    const indexed = `{${index}}`;
    if (message.includes(indexed)) {
      return message.replaceAll(indexed, argument);
    }
    return message.replace("{}", argument);
  }, template);
}

export async function setFrontendLanguage(next: Language): Promise<void> {
  useFrontendLanguageStore.setState((current) =>
    current.language === next ? current : { ...current, language: next },
  );
  if (next !== "ja" || dictionaries.ja !== undefined) return;
  japaneseLoad ??= import("@res/languages/ja.json").then(
    (module) => module.default,
  );
  try {
    const loaded = await japaneseLoad;
    dictionaries.ja = loaded;
    if (useFrontendLanguageStore.getState().language === "ja") {
      useFrontendLanguageStore.setState((current) => ({
        ...current,
        revision: current.revision + 1,
      }));
    }
  } catch (error) {
    japaneseLoad = null;
    throw error;
  }
}
