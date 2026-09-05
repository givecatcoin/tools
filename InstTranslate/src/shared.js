/*
===========================================================================
選択即時翻訳で共有する定数と設定の出入口。
============================================================================
*/
self.InstTranslate = {
  AUTO_SOURCE: "auto",

  DEFAULT_SETTINGS: {
    enabled: true,
    keepPanelAfterDeselect: false,
    sourceLanguage: "auto",
    targetLanguage: "ja",
    fontSize: 14
  },

  FONT_SIZE_MIN: 10,
  FONT_SIZE_MAX: 32,
  MIN_SELECTION_LENGTH: 2,
  KEY_DEBOUNCE_MS: 80,
  MAX_TEXT_LENGTH: 1800,
  TRANSLATE_WINDOW_WIDTH: 920,
  TRANSLATE_WINDOW_HEIGHT: 560,
  TRANSLATE_WINDOW_KEY: "translateWindowId",
  MESSAGE_TRANSLATE: "TRANSLATE_SELECTION",
  MESSAGE_SET_TEXT: "SET_SOURCE_TEXT",
  MESSAGE_FONT_SIZE: "APPLY_FONT_SIZE",

  LANGUAGE_OPTIONS: [
    { code: "ja", label: "日本語" },
    { code: "en", label: "English" },
    { code: "zh-CN", label: "简体中文" },
    { code: "zh-TW", label: "繁體中文" },
    { code: "ko", label: "한국어" },
    { code: "es", label: "Español" },
    { code: "fr", label: "Français" },
    { code: "de", label: "Deutsch" },
    { code: "it", label: "Italiano" },
    { code: "pt", label: "Português" },
    { code: "ru", label: "Русский" },
    { code: "ar", label: "العربية" },
    { code: "hi", label: "हिन्दी" },
    { code: "th", label: "ไทย" },
    { code: "vi", label: "Tiếng Việt" },
    { code: "id", label: "Bahasa Indonesia" }
  ],

  /*
  ===========================================================================
  保存済み設定を読み、欠けている項目は初期値で埋める。
  ============================================================================
  */
  loadSettings: async function loadSettings() {
    const [localStored, syncStored] = await Promise.all([
      chrome.storage.local.get(null),
      chrome.storage.sync.get(null)
    ]);
    const stored = Object.assign({}, syncStored, localStored);

    return {
      enabled: stored.enabled !== false,
      keepPanelAfterDeselect: stored.keepPanelAfterDeselect === true,
      sourceLanguage: stored.sourceLanguage || this.DEFAULT_SETTINGS.sourceLanguage,
      targetLanguage: stored.targetLanguage || this.DEFAULT_SETTINGS.targetLanguage,
      fontSize: this.normalizeFontSize(
        stored.fontSize == null ? this.DEFAULT_SETTINGS.fontSize : stored.fontSize
      )
    };
  },

  /*
  ===========================================================================
  文字サイズを、用意した段階のいずれかに揃える。
  ============================================================================
  */
  normalizeFontSize: function normalizeFontSize(value) {
    const size = Math.round(Number(value));
    if (!size) {
      return this.DEFAULT_SETTINGS.fontSize;
    }

    if (size < this.FONT_SIZE_MIN) {
      return this.FONT_SIZE_MIN;
    }

    if (size > this.FONT_SIZE_MAX) {
      return this.FONT_SIZE_MAX;
    }

    return size;
  },

  /*
  ===========================================================================
  渡された項目だけを、この端末と同期先の両方へ書き込む。
  ============================================================================
  */
  saveSettings: async function saveSettings(partial) {
    await chrome.storage.local.set(partial);

    try {
      await chrome.storage.sync.set(partial);
    } catch (error) {
      // 同期保存が使えない端末でも、この端末の設定は残す。
    }
  },

  /*
  ===========================================================================
  Google 翻訳の公式ページ URL を組み立てる。
  ============================================================================
  */
  buildTranslateUrl: function buildTranslateUrl(text, sourceLanguage, targetLanguage) {
    const clipped = String(text).slice(0, this.MAX_TEXT_LENGTH);
    const params = new URLSearchParams({
      sl: sourceLanguage && sourceLanguage !== this.AUTO_SOURCE ? sourceLanguage : "auto",
      tl: targetLanguage,
      text: clipped,
      op: "translate"
    });
    return "https://translate.google.com/?" + params.toString();
  }
};
