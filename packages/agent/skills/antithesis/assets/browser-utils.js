(function () {
  function clean(text) {
    return (text || "").replace(/\s+/g, " ").trim();
  }

  function wait(ms) {
    return new Promise(function (resolve) {
      setTimeout(resolve, ms);
    });
  }

  function isVisible(el) {
    if (!el || typeof el.getBoundingClientRect !== "function") return false;
    var rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    var style = window.getComputedStyle(el);
    return style.display !== "none" && style.visibility !== "hidden";
  }

  function click(el) {
    if (!el) return false;
    ["pointerdown", "mousedown", "mouseup", "click"].forEach(function (type) {
      el.dispatchEvent(new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        composed: true,
        view: window,
      }));
    });
    return true;
  }

  function readiness(waitForReady, getApi) {
    return function (options) {
      var api = getApi();
      return waitForReady(
        function () { return api.loadingFinished(); },
        function () { return api.loadingStatus(); },
        options,
      );
    };
  }

  window.__antithesisBrowserUtils = {
    clean: clean,
    wait: wait,
    isVisible: isVisible,
    click: click,
    readiness: readiness,
  };
})();
