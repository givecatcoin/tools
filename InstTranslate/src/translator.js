/*
===========================================================================
Chrome 内蔵の言語検出と翻訳を使い、端末内で訳す。
============================================================================
*/
self.InstTranslateEngine = {
  detector: null,
  translators: {},

  /*
  ===========================================================================
  設定の言語コードを、Translator API が受け付ける形へ揃える。
  ============================================================================
  */
  mapLanguage: function mapLanguage(code) {
    if (code === "zh-CN") {
      return "zh";
    }

    if (code === "zh-TW") {
      return "zh-Hant";
    }

    return code;
  },

  /*
  ===========================================================================
  このページで内蔵翻訳が呼べるかを見る。
  ============================================================================
  */
  isSupported: function isSupported() {
    return "Translator" in self;
  },

  /*
  ===========================================================================
  言語検出器を用意する。未ダウンロードなら進捗を知らせる。
  ============================================================================
  */
  ensureDetector: async function ensureDetector(onProgress) {
    if (this.detector) {
      return this.detector;
    }

    const availability = await LanguageDetector.availability();
    if (availability === "unavailable") {
      return null;
    }

    this.detector = await LanguageDetector.create({
      monitor: function monitorDetector(monitor) {
        monitor.addEventListener("downloadprogress", function onProgressEvent(event) {
          if (onProgress) {
            onProgress(Math.round(event.loaded * 100));
          }
        });
      }
    });

    return this.detector;
  },

  /*
  ===========================================================================
  言語ペアごとの翻訳器を用意する。
  ============================================================================
  */
  ensureTranslator: async function ensureTranslator(sourceLanguage, targetLanguage, onProgress) {
    const key = sourceLanguage + ":" + targetLanguage;
    if (this.translators[key]) {
      return this.translators[key];
    }

    const availability = await Translator.availability({
      sourceLanguage: sourceLanguage,
      targetLanguage: targetLanguage
    });

    if (availability === "unavailable") {
      return null;
    }

    this.translators[key] = await Translator.create({
      sourceLanguage: sourceLanguage,
      targetLanguage: targetLanguage,
      monitor: function monitorTranslator(monitor) {
        monitor.addEventListener("downloadprogress", function onProgressEvent(event) {
          if (onProgress) {
            onProgress(Math.round(event.loaded * 100));
          }
        });
      }
    });

    return this.translators[key];
  },

  /*
  ===========================================================================
  自動判別のときは検出器を使い、指定言語のときはそのコードを使う。
  ============================================================================
  */
  resolveSourceLanguage: async function resolveSourceLanguage(text, sourceLanguage, onProgress) {
    if (sourceLanguage && sourceLanguage !== self.InstTranslate.AUTO_SOURCE) {
      return this.mapLanguage(sourceLanguage);
    }

    if (!("LanguageDetector" in self)) {
      return null;
    }

    const detector = await this.ensureDetector(onProgress);
    if (!detector) {
      return null;
    }

    const detectedList = await detector.detect(text);
    const detected = detectedList && detectedList[0] ? detectedList[0].detectedLanguage : "";
    return detected && detected !== "und" ? detected : "en";
  },

  /*
  ===========================================================================
  選択テキストを端末内で翻訳する。使えなければ失敗を返す。
  ============================================================================
  */
  translate: async function translate(text, sourceLanguage, targetLanguage, onProgress) {
    if (!this.isSupported()) {
      return { ok: false, reason: "unsupported" };
    }

    try {
      const resolvedSource = await this.resolveSourceLanguage(text, sourceLanguage, onProgress);
      if (!resolvedSource) {
        return { ok: false, reason: "detector-unavailable" };
      }

      const mappedTarget = this.mapLanguage(targetLanguage);
      const sourceForPair = resolvedSource;

      if (sourceForPair === mappedTarget) {
        return { ok: true, text: text, sameLanguage: true };
      }

      const translator = await this.ensureTranslator(sourceForPair, mappedTarget, onProgress);
      if (!translator) {
        return { ok: false, reason: "translator-unavailable" };
      }

      const translated = await translator.translate(text);
      return { ok: true, text: translated, sameLanguage: false };
    } catch (error) {
      return { ok: false, reason: "error" };
    }
  }
};
