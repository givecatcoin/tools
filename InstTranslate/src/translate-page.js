/*
===========================================================================
開いている Google 翻訳ページへ、選択テキストを再読み込みなしで流し込む。
============================================================================
*/
(function initTranslatePage() {
  const shared = self.InstTranslate;

  /*
  ===========================================================================
  原文入力欄を探す。ラベルや jsname が変わっても、最初の編集可能な欄を使う。
  ============================================================================
  */
  function findSourceTextarea() {
    const labeled = document.querySelector(
      'textarea[aria-label="原文"], textarea[aria-label="Source text"], textarea[aria-label="Texto original"], textarea[jsname="BJE2fc"]'
    );

    if (labeled) {
      return labeled;
    }

    const areas = Array.from(document.querySelectorAll("textarea"));
    return areas.find(function isEditableArea(area) {
      return !area.readOnly && !area.disabled && area.offsetParent !== null;
    }) || null;
  }

  /*
  ===========================================================================
  原文欄へテキストを入れ、Google 側の入力イベントを発火させる。
  ============================================================================
  */
  function writeSourceText(textarea, text) {
    const descriptor = Object.getOwnPropertyDescriptor(
      window.HTMLTextAreaElement.prototype,
      "value"
    );

    if (descriptor && descriptor.set) {
      descriptor.set.call(textarea, text);
    } else {
      textarea.value = text;
    }

    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    textarea.dispatchEvent(new Event("change", { bubbles: true }));
  }

  /*
  ===========================================================================
  言語一覧や余分なパネルが前面に出ているときは閉じる。
  ============================================================================
  */
  function dismissBlockingUi() {
    const closeNames = {
      "Close picker": true,
      "Close": true,
      "Done": true,
      "閉じる": true,
      "完了": true
    };

    Array.from(document.querySelectorAll("button")).forEach(function clickCloser(button) {
      const name = (button.getAttribute("aria-label") || button.textContent || "").trim();
      if (!closeNames[name]) {
        return;
      }

      if (button.closest('[role="dialog"], [aria-modal="true"]')) {
        button.click();
      }
    });
  }

  /*
  ===========================================================================
  履歴・保存済みなど、翻訳結果の周りにある日本語の余分配列を隠す。
  ============================================================================
  */
  function hideExtraJapaneseChrome() {
    const hideNames = {
      "Saved": true,
      "History": true,
      "保存済み": true,
      "履歴": true,
      "Send feedback": true,
      "フィードバックを送信": true,
      "Open AI tools": true,
      "AI ツールを開く": true
    };

    Array.from(document.querySelectorAll("a, button")).forEach(function hideNamed(node) {
      const name = (node.getAttribute("aria-label") || node.textContent || "").trim();
      if (!hideNames[name]) {
        return;
      }

      const block = node.closest("nav, aside, header, footer") || node;
      block.style.setProperty("display", "none", "important");
    });
  }

  /*
  ===========================================================================
  原文欄が現れるまで短く待つ。初回表示の読み込み中に使う。
  ============================================================================
  */
  function waitForTextarea(timeoutMs) {
    return new Promise(function wait(resolve) {
      const first = findSourceTextarea();
      if (first) {
        resolve(first);
        return;
      }

      const startedAt = Date.now();
      const timer = window.setInterval(function poll() {
        const found = findSourceTextarea();
        if (found || Date.now() - startedAt > timeoutMs) {
          window.clearInterval(timer);
          resolve(found);
        }
      }, 50);
    });
  }

  /*
  ===========================================================================
  選択テキストを原文欄へ反映する。失敗したら background が URL 更新に戻す。
  ============================================================================
  */
  async function applySourceText(text) {
    dismissBlockingUi();
    hideExtraJapaneseChrome();

    const textarea = await waitForTextarea(1500);
    if (!textarea) {
      return false;
    }

    writeSourceText(textarea, text);
    return true;
  }

  hideExtraJapaneseChrome();
  dismissBlockingUi();

  chrome.runtime.onMessage.addListener(function onMessage(message, _sender, sendResponse) {
    if (!message || message.type !== shared.MESSAGE_SET_TEXT || !message.text) {
      return;
    }

    applySourceText(message.text).then(function reply(ok) {
      sendResponse({ ok: ok });
    });

    return true;
  });
})();
