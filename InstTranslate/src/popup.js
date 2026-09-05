/*
===========================================================================
ツールバー用の設定画面。有効状態と翻訳元・翻訳先言語を扱う。
============================================================================
*/
(function initPopup() {
  const shared = self.InstTranslate;
  const enabledInput = document.getElementById("enabled");
  const keepPanelInput = document.getElementById("keepPanelAfterDeselect");
  const sourceSelect = document.getElementById("sourceLanguage");
  const targetSelect = document.getElementById("targetLanguage");
  const helpDetails = document.querySelector(".help");

  /*
  ===========================================================================
  使い方を開いたとき、ポップアップの高さが足りなくなるのを防ぐ。
  ============================================================================
  */
  function syncPopupHeight() {
    document.body.style.minHeight = helpDetails.open ? "400px" : "260px";
  }

  /*
  ===========================================================================
  言語の選択肢を設定画面へ並べる。翻訳元は先頭に自動判別を置く。
  ============================================================================
  */
  function fillLanguageOptions(selectNode, selectedCode, includeAuto) {
    selectNode.replaceChildren();

    if (includeAuto) {
      const autoOption = document.createElement("option");
      autoOption.value = shared.AUTO_SOURCE;
      autoOption.textContent = "自動判別";
      autoOption.selected = selectedCode === shared.AUTO_SOURCE;
      selectNode.appendChild(autoOption);
    }

    shared.LANGUAGE_OPTIONS.forEach(function appendOption(language) {
      const option = document.createElement("option");
      option.value = language.code;
      option.textContent = language.label;
      option.selected = language.code === selectedCode;
      selectNode.appendChild(option);
    });
  }

  shared.loadSettings().then(function applySettings(settings) {
    enabledInput.checked = settings.enabled;
    keepPanelInput.checked = settings.keepPanelAfterDeselect;
    fillLanguageOptions(sourceSelect, settings.sourceLanguage, true);
    fillLanguageOptions(targetSelect, settings.targetLanguage, false);
  });

  enabledInput.addEventListener("change", function onEnabledChange() {
    shared.saveSettings({ enabled: enabledInput.checked });
  });

  keepPanelInput.addEventListener("change", function onKeepPanelChange() {
    shared.saveSettings({ keepPanelAfterDeselect: keepPanelInput.checked });
  });

  sourceSelect.addEventListener("change", function onSourceChange() {
    shared.saveSettings({ sourceLanguage: sourceSelect.value });
  });

  targetSelect.addEventListener("change", function onTargetChange() {
    shared.saveSettings({ targetLanguage: targetSelect.value });
  });

  helpDetails.addEventListener("toggle", syncPopupHeight);
  syncPopupHeight();
})();
