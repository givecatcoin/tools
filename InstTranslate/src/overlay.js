/*
===========================================================================
選択時に Chrome 右上へ出す、常駐の翻訳パネル。
============================================================================
*/
self.InstTranslateOverlay = (function createOverlay() {
  const HOST_ID = "insttranslate-host";
  let host = null;
  let root = null;
  let statusNode = null;
  let resultNode = null;
  let sizeStyle = null;

  const STYLES = [
    ":host {",
    "  all: initial;",
    "  font-size: medium !important;",
    "}",
    "#panel {",
    "  position: fixed;",
    "  top: 16px;",
    "  right: 16px;",
    "  z-index: 2147483646;",
    "  width: fit-content;",
    "  min-width: 140px;",
    "  max-width: min(480px, calc(100vw - 32px));",
    "  height: auto;",
    "  max-height: 42vh;",
    "  overflow: auto;",
    "  box-sizing: border-box;",
    "  padding: 10px 12px;",
    "  border-radius: 12px;",
    "  background: #fff;",
    "  color: #202124;",
    "  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.18);",
    "  font-family: 'Segoe UI', 'Hiragino Sans', sans-serif;",
    "  line-height: 1.55;",
    "}",
    "header { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 6px; }",
    "strong { font-size: 12px; color: #5f6368; }",
    "button { border: 0; background: transparent; color: #5f6368; cursor: pointer; font-size: 16px; line-height: 1; }",
    "#status { margin: 0; color: #5f6368; font-size: 12px; }",
    "#status[hidden], #result[hidden] { display: none; }",
    "#result { margin: 0; white-space: pre-wrap; word-break: break-word; }"
  ].join("");

  let resultFontSize = 14;

  /*
  ===========================================================================
  パネル用の Shadow DOM を一度だけ作る。
  ============================================================================
  */
  function ensurePanel() {
    if (host && document.documentElement.contains(host)) {
      return;
    }

    host = document.createElement("div");
    host.id = HOST_ID;
    root = host.attachShadow({ mode: "closed" });

    const style = document.createElement("style");
    style.textContent = STYLES;

    const panel = document.createElement("div");
    panel.id = "panel";

    const header = document.createElement("header");
    const title = document.createElement("strong");
    title.textContent = "InstTranslate";

    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.setAttribute("aria-label", "閉じる");
    closeButton.textContent = "×";
    closeButton.addEventListener("click", hide);

    header.appendChild(title);
    header.appendChild(closeButton);

    statusNode = document.createElement("p");
    statusNode.id = "status";

    resultNode = document.createElement("p");
    resultNode.id = "result";

    sizeStyle = document.createElement("style");

    panel.appendChild(header);
    panel.appendChild(statusNode);
    panel.appendChild(resultNode);
    root.appendChild(style);
    root.appendChild(sizeStyle);
    root.appendChild(panel);
    document.documentElement.appendChild(host);
    applyFontSize();
  }

  /*
  ===========================================================================
  訳文の CSS に文字サイズを書き込み、他の指定より優先する。
  ============================================================================
  */
  function applyFontSize() {
    const css = [
      "#panel, #result {",
      "  font-size: " + resultFontSize + "px !important;",
      "  line-height: 1.55 !important;",
      "}"
    ].join("");

    if (sizeStyle) {
      sizeStyle.textContent = css;
    }

    if (resultNode) {
      resultNode.setAttribute(
        "style",
        "font-size: " + resultFontSize + "px !important; line-height: 1.55 !important;"
      );
    }
  }

  /*
  ===========================================================================
  訳文の文字サイズを変え、表示中ならすぐ反映する。
  ============================================================================
  */
  function setFontSize(size) {
    const next = Number(size);
    if (!next) {
      return;
    }

    resultFontSize = next;
    applyFontSize();
  }

  /*
  ===========================================================================
  右上パネルを出し、訳文または状態だけを書き込む。
  ============================================================================
  */
  function show(view) {
    ensurePanel();
    if (view.fontSize != null && view.fontSize !== "") {
      resultFontSize = Number(view.fontSize) || resultFontSize;
    }

    applyFontSize();
    host.style.display = "block";
    statusNode.textContent = view.status || "";
    statusNode.hidden = !view.status;
    resultNode.textContent = view.result || "";
    resultNode.hidden = !view.result;
  }

  /*
  ===========================================================================
  右上パネルを隠す。
  ============================================================================
  */
  function hide() {
    if (host) {
      host.style.display = "none";
    }
  }

  /*
  ===========================================================================
  クリックや選択がこのパネル上かを見る。
  ============================================================================
  */
  function containsEvent(event) {
    if (!host) {
      return false;
    }

    const path = event.composedPath ? event.composedPath() : [];
    return path.indexOf(host) !== -1;
  }

  document.addEventListener("keydown", function onKeyDown(event) {
    if (event.key === "Escape") {
      hide();
    }
  }, true);

  return {
    show: show,
    hide: hide,
    setFontSize: setFontSize,
    containsEvent: containsEvent
  };
})();
