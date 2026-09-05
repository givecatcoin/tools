/*
===========================================================================
選択テキストを受け取り、Google 翻訳の小さな窓を開くか更新する。
============================================================================
*/
importScripts("shared.js");

const shared = self.InstTranslate;

/*
===========================================================================
ツールバーアイコンのバッジを、有効状態に合わせて更新する。
============================================================================
*/
async function refreshActionBadge() {
  const settings = await shared.loadSettings();
  await chrome.action.setBadgeBackgroundColor({ color: "#5f6368" });
  await chrome.action.setBadgeText({
    text: settings.enabled ? "" : "OFF"
  });
}

/*
===========================================================================
保存済みの翻訳窓 ID を読み、すでに閉じられていれば捨てる。
============================================================================
*/
async function readTranslateWindowId() {
  const stored = await chrome.storage.session.get(shared.TRANSLATE_WINDOW_KEY);
  const windowId = stored[shared.TRANSLATE_WINDOW_KEY];

  if (typeof windowId !== "number") {
    return null;
  }

  try {
    await chrome.windows.get(windowId);
    return windowId;
  } catch (error) {
    await chrome.storage.session.remove(shared.TRANSLATE_WINDOW_KEY);
    return null;
  }
}

/*
===========================================================================
指定ミリ秒だけ待つ。
============================================================================
*/
function wait(ms) {
  return new Promise(function onWait(resolve) {
    setTimeout(resolve, ms);
  });
}

/*
===========================================================================
既存の Google 翻訳タブへ、再読み込みせずテキストだけを渡す。
============================================================================
*/
async function fillExistingTranslatePage(tabId, text, targetLanguage) {
  try {
    const response = await chrome.tabs.sendMessage(tabId, {
      type: shared.MESSAGE_SET_TEXT,
      text: text,
      targetLanguage: targetLanguage
    });
    return Boolean(response && response.ok);
  } catch (error) {
    return false;
  }
}

/*
===========================================================================
記事側の窓へフォーカスを戻し、読み続けられるようにする。
============================================================================
*/
async function returnFocus(windowId) {
  if (typeof windowId !== "number") {
    return;
  }

  try {
    await chrome.windows.update(windowId, { focused: true });
  } catch (error) {
    // 元の窓が無いときは、翻訳窓が前面のままでよい。
  }
}

/*
===========================================================================
既存の翻訳窓があれば同じ窓を更新し、なければ新しく開く。
============================================================================
*/
async function openOrUpdateTranslateWindow(text) {
  const settings = await shared.loadSettings();
  if (!settings.enabled) {
    return;
  }

  const url = shared.buildTranslateUrl(text, settings.sourceLanguage, settings.targetLanguage);
  const existingId = await readTranslateWindowId();
  let readingWindowId = null;

  try {
    const current = await chrome.windows.getLastFocused();
    readingWindowId = current.id;
  } catch (error) {
    readingWindowId = null;
  }

  if (existingId !== null) {
    try {
      const tabs = await chrome.tabs.query({ windowId: existingId });
      if (tabs[0] && tabs[0].id) {
        let updated = await fillExistingTranslatePage(
          tabs[0].id,
          text,
          settings.targetLanguage
        );

        if (!updated) {
          await wait(250);
          updated = await fillExistingTranslatePage(
            tabs[0].id,
            text,
            settings.targetLanguage
          );
        }

        if (!updated) {
          await chrome.tabs.update(tabs[0].id, { url: url });
        }

        await returnFocus(readingWindowId);
        return;
      }
    } catch (error) {
      await chrome.storage.session.remove(shared.TRANSLATE_WINDOW_KEY);
    }
  }

  let left = 80;
  let top = 80;

  try {
    const current = await chrome.windows.getLastFocused();
    left = Math.max(0, current.left + current.width - shared.TRANSLATE_WINDOW_WIDTH - 20);
    top = Math.max(0, current.top + 80);
  } catch (error) {
    // 位置が取れなくても、既定位置で翻訳窓は開く。
  }

  const created = await chrome.windows.create({
    url: url,
    type: "popup",
    width: shared.TRANSLATE_WINDOW_WIDTH,
    height: shared.TRANSLATE_WINDOW_HEIGHT,
    left: left,
    top: top,
    focused: false
  });

  if (typeof created.id === "number") {
    await chrome.storage.session.set({
      [shared.TRANSLATE_WINDOW_KEY]: created.id
    });
  }

  await returnFocus(readingWindowId);
}

chrome.runtime.onInstalled.addListener(function onInstalled() {
  refreshActionBadge();
});

chrome.runtime.onStartup.addListener(function onStartup() {
  refreshActionBadge();
});

chrome.storage.onChanged.addListener(function onStorageChanged(changes, areaName) {
  if (areaName === "sync" && changes.enabled) {
    refreshActionBadge();
  }
});

chrome.windows.onRemoved.addListener(function onWindowRemoved(windowId) {
  chrome.storage.session.get(shared.TRANSLATE_WINDOW_KEY, function onLoaded(stored) {
    if (stored[shared.TRANSLATE_WINDOW_KEY] === windowId) {
      chrome.storage.session.remove(shared.TRANSLATE_WINDOW_KEY);
    }
  });
});

chrome.runtime.onMessage.addListener(function onMessage(message, _sender, sendResponse) {
  if (!message || message.type !== shared.MESSAGE_TRANSLATE || !message.text) {
    sendResponse({ ok: false });
    return;
  }

  openOrUpdateTranslateWindow(message.text);
  sendResponse({ ok: true });
});
