/*
===========================================================================
選択確定を検知し、右上パネルでローカル翻訳、だめなら Google 翻訳窓へ渡す。
============================================================================
*/
(function initContentScript() {
  const shared = self.InstTranslate;
  const engine = self.InstTranslateEngine;
  const overlay = self.InstTranslateOverlay;

  if (!shared || !engine || !overlay) {
    return;
  }

  let debounceTimer = null;
  let lastText = "";
  let requestSeq = 0;
  let cachedSettings = Object.assign({}, shared.DEFAULT_SETTINGS);

  try {
    shared.loadSettings().then(function onSettingsLoaded(settings) {
      cachedSettings = settings;
      overlay.setFontSize(cachedSettings.fontSize);
    });
  } catch (error) {
    // 拡張機能の再読み込み直後は storage が切れていることがある。
  }

  chrome.storage.onChanged.addListener(function onSettingsChanged(changes, areaName) {
    if (areaName !== "sync" && areaName !== "local") {
      return;
    }

    if (changes.enabled) {
      cachedSettings.enabled = changes.enabled.newValue !== false;
      if (cachedSettings.enabled === false) {
        overlay.hide();
      }
    }

    if (changes.keepPanelAfterDeselect) {
      cachedSettings.keepPanelAfterDeselect = changes.keepPanelAfterDeselect.newValue === true;
    }

    if (changes.sourceLanguage && changes.sourceLanguage.newValue) {
      cachedSettings.sourceLanguage = changes.sourceLanguage.newValue;
      lastText = "";
    }

    if (changes.targetLanguage && changes.targetLanguage.newValue) {
      cachedSettings.targetLanguage = changes.targetLanguage.newValue;
      lastText = "";
    }

    if (changes.fontSize) {
      cachedSettings.fontSize = shared.normalizeFontSize(changes.fontSize.newValue);
      overlay.setFontSize(cachedSettings.fontSize);
    }
  });

  chrome.runtime.onMessage.addListener(function onRuntimeMessage(message) {
    if (!message || message.type !== shared.MESSAGE_FONT_SIZE) {
      return;
    }

    cachedSettings.fontSize = shared.normalizeFontSize(message.fontSize);
    overlay.setFontSize(cachedSettings.fontSize);
  });

  /*
  ===========================================================================
  選択ノードが入力欄など、翻訳対象外の編集領域かを判定する。
  ============================================================================
  */
  function isEditableNode(node) {
    if (!node) {
      return false;
    }

    const element = node.nodeType === Node.ELEMENT_NODE
      ? node
      : node.parentElement;

    if (!element || !element.closest) {
      return false;
    }

    return Boolean(
      element.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]')
    );
  }

  /*
  ===========================================================================
  現在の選択が翻訳を起動してよいかを判定する。
  ============================================================================
  */
  function shouldIgnoreSelection(selection) {
    try {
      if (location.hostname === "translate.google.com" || location.hostname === "translate.google.co.jp") {
        return true;
      }

      if (!selection || selection.isCollapsed || !selection.rangeCount) {
        return true;
      }

      return isEditableNode(selection.anchorNode)
        || isEditableNode(selection.focusNode)
        || isEditableNode(document.activeElement);
    } catch (error) {
      return true;
    }
  }

  /*
  ===========================================================================
  確定した選択テキストを取り出す。条件を満たさなければ空文字を返す。
  ============================================================================
  */
  function readSelectedText() {
    try {
      const selection = window.getSelection();
      if (shouldIgnoreSelection(selection)) {
        return "";
      }

      const raw = selection && typeof selection.toString === "function"
        ? selection.toString()
        : "";
      const text = String(raw).replace(/\s+/g, " ").trim();
      if (text.length < shared.MIN_SELECTION_LENGTH) {
        return "";
      }

      return text.slice(0, shared.MAX_TEXT_LENGTH);
    } catch (error) {
      return "";
    }
  }

  /*
  ===========================================================================
  拡張機能の実行文脈が切れていないかを見る。
  ============================================================================
  */
  function hasLiveRuntime() {
    try {
      return Boolean(chrome.runtime && chrome.runtime.id);
    } catch (error) {
      return false;
    }
  }

  /*
  ===========================================================================
  ローカル翻訳できないときだけ、Google 翻訳窓の起動を依頼する。
  ============================================================================
  */
  function requestGoogleTranslate(text) {
    if (!hasLiveRuntime()) {
      lastText = "";
      return;
    }

    try {
      chrome.runtime.sendMessage({
        type: shared.MESSAGE_TRANSLATE,
        text: text
      }, function onMessageSent() {
        if (chrome.runtime.lastError) {
          lastText = "";
        }
      });
    } catch (error) {
      lastText = "";
    }
  }

  /*
  ===========================================================================
  選択テキストを右上へ出し、内蔵翻訳を試してから必要なら Google へ倒す。
  ============================================================================
  */
  async function translateSelection(text) {
    const seq = ++requestSeq;

    try {
      const latest = await shared.loadSettings();
      cachedSettings.fontSize = latest.fontSize;
    } catch (error) {
      // 保存読みに失敗しても、手元の設定で続ける。
    }

    try {
      overlay.show({
        status: "翻訳中…",
        fontSize: cachedSettings.fontSize
      });
    } catch (error) {
      return;
    }

    try {
      const local = await engine.translate(
        text,
        cachedSettings.sourceLanguage,
        cachedSettings.targetLanguage,
        function onProgress(percent) {
          if (seq !== requestSeq) {
            return;
          }

          overlay.show({
            status: "初回モデル準備中 " + percent + "%",
            fontSize: cachedSettings.fontSize
          });
        }
      );

      if (seq !== requestSeq) {
        return;
      }

      if (local.ok) {
        if (local.sameLanguage || !local.text) {
          overlay.hide();
          return;
        }

        overlay.show({
          result: local.text,
          fontSize: cachedSettings.fontSize
        });
        return;
      }
    } catch (error) {
      if (seq !== requestSeq) {
        return;
      }
    }

    overlay.hide();
    requestGoogleTranslate(text);
  }

  /*
  ===========================================================================
  設定で残すがオフのときだけ、選択解除でパネルを閉じる。
  ============================================================================
  */
  function dismissIfAllowed() {
    if (cachedSettings.keepPanelAfterDeselect) {
      return;
    }

    dismissOverlay();
  }

  /*
  ===========================================================================
  進行中の翻訳を打ち切り、パネルを閉じる。
  ============================================================================
  */
  function dismissOverlay() {
    lastText = "";
    requestSeq += 1;
    try {
      overlay.hide();
    } catch (error) {
      // パネル未作成や再読み込み直後は無視する。
    }
  }

  /*
  ===========================================================================
  選択が翻訳対象なら、待たずに翻訳を始める。空ならパネルを閉じる。
  ============================================================================
  */
  function fireTranslate(event) {
    if (cachedSettings.enabled === false) {
      return;
    }

    if (event && overlay.containsEvent(event)) {
      return;
    }

    const text = readSelectedText();
    if (!text) {
      dismissIfAllowed();
      return;
    }

    if (text === lastText) {
      return;
    }

    lastText = text;
    translateSelection(text).catch(function ignoreTranslateFailure() {
      // 翻訳 API や描画の失敗をページへ漏らさない。
    });
  }

  document.addEventListener("selectionchange", function onSelectionChange() {
    try {
      if (!readSelectedText()) {
        dismissIfAllowed();
      }
    } catch (error) {
      // 選択オブジェクトが途中で無効になっても無視する。
    }
  });

  document.addEventListener("mouseup", function onMouseUp(event) {
    if (event.button !== 0) {
      return;
    }

    fireTranslate(event);
  }, true);

  document.addEventListener("keyup", function onKeyUp(event) {
    const selectionKeys = {
      ArrowLeft: true,
      ArrowRight: true,
      ArrowUp: true,
      ArrowDown: true,
      Home: true,
      End: true
    };

    if (!event.shiftKey || !selectionKeys[event.key]) {
      return;
    }

    window.clearTimeout(debounceTimer);
    debounceTimer = window.setTimeout(function onKeySettled() {
      fireTranslate(event);
    }, shared.KEY_DEBOUNCE_MS);
  }, true);
})();
