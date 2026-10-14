/* 桌面版存储桥接层
 *
 * The page talks to `window.storage` — an async get/set/list API. In the artifact build that
 * object is provided by the host; here it is backed by a JSON file on disk through the Rust
 * side. Shapes match exactly ({value} / {keys}), so not a single line of the application code
 * changes between the two builds, and the two cannot drift apart.
 *
 * One deliberate difference in behaviour: reading a key that was never written returns
 * {value: null} instead of throwing. The web API raised in that case, which is why the page
 * carries a listing-then-get fallback; that path simply never triggers here.
 */
(function () {
  "use strict";

  // Tauri v2 exposes either of these depending on how the app was configured.
  function getInvoke() {
    if (window.__TAURI__ && window.__TAURI__.core && typeof window.__TAURI__.core.invoke === "function") {
      return window.__TAURI__.core.invoke;
    }
    if (window.__TAURI_INTERNALS__ && typeof window.__TAURI_INTERNALS__.invoke === "function") {
      return window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
    }
    return null;
  }

  var invoke = getInvoke();
  if (!invoke) return;   // not running inside the desktop shell — leave window.storage alone

  window.__PM_DESKTOP__ = true;

  window.storage = {
    get: function (key) {
      return invoke("storage_get", { key: String(key) }).then(function (value) {
        return { value: value === undefined ? null : value };
      });
    },
    set: function (key, value) {
      return invoke("storage_set", { key: String(key), value: String(value) }).then(function () {
        return true;
      });
    },
    delete: function (key) {
      return invoke("storage_delete", { key: String(key) }).then(function () {
        return true;
      });
    },
    list: function () {
      return invoke("storage_list").then(function (keys) {
        return { keys: keys || [] };
      });
    }
  };

  /* Hand a generated file to the native save dialog.
   *
   * Returns the saved path, or null when the person cancels. Callers fall back to the
   * browser download if this is absent, so the same page still works outside the shell. */
  window.__PM_SAVE_FILE__ = function (suggestedName, bytes) {
    return invoke("save_file", {
      suggestedName: String(suggestedName),
      bytes: Array.from(bytes)
    });
  };

  /* Replace the web build's storage banner with where the data actually is.
   *
   * On a portable app this is the single most useful thing to show: the person needs to know
   * which folder to back up or copy, and — if the exe landed somewhere unwritable and the app
   * fell back to %APPDATA% — that the folder beside the exe is *not* their data. */
  window.__PM_STORAGE_INFO__ = invoke("storage_info").then(
    function (info) { return info; },
    function () { return null; }
  );

  function showLocation(info) {
    if (!info) return;
    var portable = info.location === "portable";
    var cls = portable ? "storage-banner ok" : "storage-banner warn";
    var text = portable
      ? "数据目录：" + info.path + "　（自动保留最近 " + info.backups_kept + " 个版本备份）"
      : "⚠ 程序所在目录不可写，数据已改存到：" + info.path +
        "　（把程序移到有写权限的位置，数据才会跟着程序走）";
    // Stashed rather than written directly: the app re-renders this banner on every save, and
    // its healthy branch now reads from here.
    window.__PM_DATA_BANNER__ = '<div class="' + cls + '">' + escapeText(text) + "</div>";
    var el = document.getElementById("storageBanner");
    if (el && typeof window.updateStorageBanner === "function") window.updateStorageBanner();
    else if (el) el.innerHTML = window.__PM_DATA_BANNER__;
  }

  function escapeText(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      window.__PM_STORAGE_INFO__.then(showLocation);
    }, { once: true });
  } else {
    window.__PM_STORAGE_INFO__.then(showLocation);
  }
})();
