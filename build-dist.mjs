/* Turn the web build into the desktop build.
 *
 * The single-file web app stays the one source of truth; this script derives dist/index.html
 * from it, so a change to the web version is picked up by re-running `npm run dist` rather
 * than being hand-merged into a diverging copy.
 *
 * Three transforms, each one asserted so the build fails loudly if the web version moves out
 * from under it instead of silently producing a broken app:
 *   1. inject the storage bridge before the application script
 *   2. drop the Google Fonts loader — a portable app must not wait on the network, and on a
 *      locked-down intranet machine that request just times out
 *   3. disable the browser context menu and text selection chrome that make a desktop window
 *      feel like a web page
 */
import { readFileSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
// 仓库内（含 CI）网页版与本脚本同级；旧的本地目录布局里它在上一级，保留为回退
const sibling = resolve(here, "pm-dashboard-v2.html");
const SRC = process.argv[2] || (existsSync(sibling) ? sibling : resolve(here, "../pm-dashboard-v2.html"));
const OUT = resolve(here, "dist/index.html");

let html = readFileSync(SRC, "utf8");
const before = html.length;

function must(condition, message) {
  if (!condition) {
    console.error("构建失败：" + message);
    console.error("网页版结构可能已变化，请更新 build-dist.mjs 后重试。");
    process.exit(1);
  }
}

/* ---- 1. system fonts instead of the Google Fonts request ---------------------------- */
// The web build attaches the stylesheet from a script tag after parsing. Remove that whole
// block: Windows ships 微软雅黑 and Consolas, which the font stack already falls back to.
const fontBlockStart = html.indexOf('<link rel="preconnect" href="https://fonts.googleapis.com">');
must(fontBlockStart !== -1, "找不到字体 preconnect 标记");
const fontBlockEnd = html.indexOf("</script>", fontBlockStart);
must(fontBlockEnd !== -1, "找不到字体加载脚本的结束标记");
html =
  html.slice(0, fontBlockStart) +
  "<!-- 桌面版使用系统字体：不发任何网络请求，内网机器也能秒开 -->" +
  html.slice(fontBlockEnd + "</script>".length);

// Point the stacks at fonts Windows actually has, keeping the same visual intent.
must(html.includes("--font-sans:"), "找不到 --font-sans 变量");
html = html.replace(
  /--font-mono:[^;]+;/,
  "--font-mono:'Cascadia Mono','Consolas',ui-monospace,monospace;"
);
html = html.replace(
  /--font-sans:[^;]+;/,
  "--font-sans:'Microsoft YaHei UI','Microsoft YaHei','PingFang SC',system-ui,sans-serif;"
);

/* ---- 2. inject the storage bridge before the application script --------------------- */
// It must run before the app, which reads window.storage during start-up.
const bridge = readFileSync(resolve(here, "dist/desktop-bridge.js"), "utf8");
const appScript = html.indexOf('<script>\n(function(){\n"use strict";');
must(appScript !== -1, "找不到主程序脚本的起始位置");
html =
  html.slice(0, appScript) +
  "<script>\n" + bridge + "\n</script>\n" +
  html.slice(appScript);

/* ---- 3. desktop window chrome ------------------------------------------------------- */
const desktopCss = `
<style>
/* 桌面窗口而非网页：去掉右键菜单之外的网页感，但保留输入框内的正常选择与编辑 */
:root, body { overscroll-behavior: none; }
body { user-select: none; -webkit-user-select: none; cursor: default; }
input, textarea, select, .bar, .sd-row, .ms-row, td, th, .conflict-item {
  user-select: text; -webkit-user-select: text;
}
.storage-banner.ok { color: var(--text-dim); }
.storage-banner.warn { color: var(--accent-warn); }
</style>
<script>
// A desktop app has no use for "reload" or "view source"; keep the browser menu out of the way.
// F5 / Ctrl+R would throw away unsaved in-memory state, so they are swallowed too.
document.addEventListener('contextmenu', function (e) {
  if (!e.target.closest('input, textarea')) e.preventDefault();
});
document.addEventListener('keydown', function (e) {
  if (e.key === 'F5' || ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'r')) e.preventDefault();
});
</script>
`;
must(html.includes("</head>"), "找不到 </head>");
html = html.replace("</head>", desktopCss + "</head>");

/* ---- 4. the status bar shows where the data lives ----------------------------------- */
// In the web build this banner is only used for storage *problems* and is cleared when all is
// well. On a portable desktop app the healthy case is worth stating: the person needs to know
// which folder to back up or copy to another machine — and, if the exe landed somewhere
// unwritable and the data fell back to %APPDATA%, that the folder beside the exe is not it.
const bannerClear = 'if(storageAvailable){ banner.innerHTML = ""; return; }';
must(html.includes(bannerClear), "找不到状态栏的正常分支");
html = html.replace(
  bannerClear,
  'if(storageAvailable){ banner.innerHTML = window.__PM_DATA_BANNER__ || ""; return; }'
);

/* ---- 5. exports go through the native save dialog ----------------------------------- */
// A webview is not a browser: `XLSX.writeFile` and the Blob-plus-anchor trick either drop the
// file silently into Downloads or get blocked, and either way the person never gets to choose
// where it lands. Both export paths are rerouted to the shell's save dialog, keeping the
// original browser behaviour as the fallback so the page still works outside the desktop app.
const xlsxWrite = "    XLSX.writeFile(wb, fname);\n    showToast(\"Excel 已导出\");";
must(html.includes(xlsxWrite), "找不到 Excel 导出的写文件调用");
html = html.replace(
  xlsxWrite,
  `    if(window.__PM_SAVE_FILE__){
      var __buf = XLSX.write(wb, { bookType: "xlsx", type: "array" });
      window.__PM_SAVE_FILE__(fname, new Uint8Array(__buf)).then(function(p){
        showToast(p ? "已导出到 " + p : "已取消导出");
      }, function(err){ showToast("导出失败：" + err, true); });
    } else {
      XLSX.writeFile(wb, fname);
      showToast("Excel 已导出");
    }`
);

const jsonDownload = `  var blob = new Blob([JSON.stringify({ projects: state.projects, milestones: state.milestones }, null, 2)], { type: "application/json" });`;
must(html.includes(jsonDownload), "找不到备份导出的 Blob 构造");
html = html.replace(
  jsonDownload,
  `  var __json = JSON.stringify({ projects: state.projects, milestones: state.milestones }, null, 2);
  if(window.__PM_SAVE_FILE__){
    var __name = "milestone-dashboard-backup-" + todayISO() + ".json";
    window.__PM_SAVE_FILE__(__name, new TextEncoder().encode(__json)).then(function(p){
      showToast(p ? "备份已保存到 " + p : "已取消");
    }, function(err){ showToast("备份失败：" + err, true); });
    return;
  }
  var blob = new Blob([__json], { type: "application/json" });`
);

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, html, "utf8");

console.log("已生成 " + OUT);
console.log("  源文件 " + (before / 1024).toFixed(0) + " KB → 产物 " + (html.length / 1024).toFixed(0) + " KB");
console.log("  字体：系统自带（无网络请求）");
console.log("  存储：window.storage → Rust 本地 JSON");
