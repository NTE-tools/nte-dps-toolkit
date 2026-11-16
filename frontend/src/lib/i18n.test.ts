import { afterEach, describe, expect, it } from "vitest";

import placeholderConformance from "@res/languages/placeholder-conformance.json";

import {
  formatTemplate,
  getFrontendLanguage,
  setFrontendLanguage,
  t,
} from "./i18n";

afterEach(async () => {
  await setFrontendLanguage("zh-CN");
});

describe("frontend i18n", () => {
  it.each(placeholderConformance)(
    "formats the shared placeholder grammar: $template",
    ({ template, arguments: arguments_, expected }) => {
      expect(formatTemplate(template, arguments_)).toBe(expected);
    },
  );

  it("switches synchronously between zh-CN and en and lazy-loads ja", async () => {
    await setFrontendLanguage("en");
    expect(getFrontendLanguage()).toBe("en");
    expect(t("Settings")).toBe("Settings");

    await setFrontendLanguage("zh-CN");
    expect(getFrontendLanguage()).toBe("zh-CN");
    expect(t("Settings")).toBe("设置");

    await setFrontendLanguage("ja");
    expect(getFrontendLanguage()).toBe("ja");
    expect(t("Settings")).not.toBe("Settings");
    expect(t("Unmapped fallback key")).toBe("Unmapped fallback key");
  });
});
