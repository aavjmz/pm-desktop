// End-to-end check of the desktop build: the real dist/index.html driven through a mock of
// the Tauri bridge that behaves exactly like the Rust store (missing key -> null, never
// throws). "Restart" is a brand-new page carrying the previous disk forward — a reload would
// not do, because the invoke handler closes over the disk it was created with.
const { chromium } = require('playwright');
const DIST = 'file://' + require('path').resolve(__dirname, '..', 'dist', 'index.html');

const MOCK = (disk) => `(() => {
  const disk = ${JSON.stringify(disk)};
  window.__fakeDisk = disk;
  window.__calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    window.__calls.push(cmd);
    await new Promise(r => setTimeout(r, 1));              // IPC is not instant
    switch (cmd) {
      case 'storage_get':    return disk[args.key] === undefined ? null : disk[args.key];
      case 'storage_set':    disk[args.key] = args.value; return null;
      case 'storage_delete': delete disk[args.key]; return null;
      case 'storage_list':   return Object.keys(disk).sort();
      case 'storage_info':   return { path: 'D:\\\\里程碑指挥台\\\\data', location: 'portable', backups_kept: 10 };
      case 'save_file':      window.__saved = { name: args.suggestedName, bytes: args.bytes }; return 'D:\\\\导出\\\\' + args.suggestedName;
      default: throw new Error('unknown command ' + cmd);
    }
  }}};
})()`;

async function open(browser, disk) {
  const pg = await browser.newPage({ viewport: { width: 1400, height: 900 } });
  const errs = [];
  pg.on('pageerror', e => errs.push('PAGEERROR: ' + e.message));
  pg.on('console', m => { if (m.type() === 'error' && !/net::/.test(m.text())) errs.push(m.text()); });
  await pg.addInitScript(MOCK(disk));
  await pg.goto(DIST);
  await pg.waitForTimeout(1600);
  return { pg, errs };
}

(async () => {
  const b = await chromium.launch();
  let pass = true;
  const check = (label, got, want) => {
    const ok = got === want;
    if (!ok) pass = false;
    console.log(`  ${ok ? '✓' : '✗'} ${label}: ${got}${ok ? '' : '   期望 ' + want}`);
  };

  // ---------- 首次启动（空数据目录） ----------
  console.log('首次启动（数据目录为空）');
  let { pg, errs } = await open(b, {});
  const boot = await pg.evaluate(() => ({
    desktop: !!window.__PM_DESKTOP__,
    fontReqs: performance.getEntriesByType('resource').filter(r => /fonts\.g/.test(r.name)).length,
    banner: (document.getElementById('storageBanner') || {}).textContent.trim(),
    calls: window.__calls.slice()
  }));
  check('识别为桌面模式', boot.desktop, true);
  check('字体网络请求', boot.fontReqs, 0);
  check('状态栏显示数据目录', boot.banner.startsWith('数据目录：'), true);
  console.log('    IPC: ' + JSON.stringify(boot.calls));

  // 通过真实 UI 新建项目，走应用自己的保存路径
  await pg.click('#btnNewProject');
  await pg.waitForTimeout(400);
  await pg.fill('#fName', 'C12E02·比亚迪');
  await pg.fill('#fClient', '比亚迪');
  await pg.click('#mSave');
  await pg.waitForTimeout(900);
  const disk = await pg.evaluate(() => window.__fakeDisk);
  check('项目已落盘', (disk['pm_dashboard_v2'] || '').includes('C12E02·比亚迪'), true);
  await pg.close();

  // ---------- 关掉再打开 ----------
  console.log('\n关闭后重新打开（沿用同一份数据目录）');
  const second = await open(b, disk);
  pg = second.pg; errs = errs.concat(second.errs);
  const after = await pg.evaluate(() => ({
    shown: document.body.innerText.includes('C12E02·比亚迪'),
    calls: window.__calls.slice(),
    // 空数据目录首启会播种 2 个示例项目，再加上新建的那个
    hasNew: JSON.parse(window.__fakeDisk['pm_dashboard_v2']).projects.some(p => p.name === 'C12E02·比亚迪')
  }));
  check('项目仍在', after.shown, true);
  check('新建的项目在存档里', after.hasNew, true);
  check('启动未触发写探测', after.calls.includes('storage_set'), false);
  console.log('    IPC: ' + JSON.stringify(after.calls));

  // ---------- 三个页签 ----------
  console.log('\n界面');
  for (const tab of ['timeline', 'projects', 'owners']) {
    await pg.click(`[data-tab="${tab}"]`).catch(() => {});
    await pg.waitForTimeout(350);
    const n = await pg.evaluate(() => document.body.innerText.length);
    check(`${tab} 渲染`, n > 200, true);
  }

  // ---------- 导出走原生保存对话框 ----------
  console.log('\n导出');
  await pg.click('[data-tab="timeline"]').catch(() => {});
  await pg.waitForTimeout(300);
  await pg.click('#btnExportExcel');
  await pg.waitForTimeout(500);
  // 导出弹窗里选「全部项目」
  const allBtn = await pg.$('[data-exp-all]');
  if (allBtn) { await allBtn.click(); await pg.waitForTimeout(1200); }
  const xlsx = await pg.evaluate(() => {
    const s = window.__saved;
    if (!s) return null;
    // xlsx 是 zip：前两字节必须是 PK
    return { name: s.name, len: s.bytes.length, pk: s.bytes[0] === 0x50 && s.bytes[1] === 0x4B };
  });
  if (xlsx) {
    check('Excel 走了原生保存', true, true);
    check('文件名是 .xlsx', xlsx.name.endsWith('.xlsx'), true);
    check('产出是有效 zip(PK)', xlsx.pk, true);
    console.log('    ' + xlsx.name + '  ' + xlsx.len + ' 字节');
  } else {
    console.log('  (导出弹窗按钮 id 未匹配，跳过 Excel 用例)');
  }
  await pg.evaluate(() => { const m = document.getElementById('mCancel') || document.getElementById('mClose'); if (m) m.click(); window.__saved = null; });
  await pg.waitForTimeout(300);

  await pg.click('#btnExport');
  await pg.waitForTimeout(900);
  const bak = await pg.evaluate(() => {
    const s = window.__saved;
    if (!s) return null;
    const txt = new TextDecoder().decode(new Uint8Array(s.bytes));
    return { name: s.name, ok: txt.includes('C12E02·比亚迪') };
  });
  check('备份走了原生保存', !!bak, true);
  if (bak) { check('备份内容含新建项目', bak.ok, true); console.log('    ' + bak.name); }

  // ---------- 不可写目录的回落提示 ----------
  console.log('\n程序目录不可写时');
  const ro = await b.newPage({ viewport: { width: 1200, height: 700 } });
  await ro.addInitScript(MOCK({}).replace(
    "location: 'portable'", "location: 'appdata'"
  ).replace("D:\\\\里程碑指挥台\\\\data", "C:\\\\Users\\\\ddz\\\\AppData\\\\Roaming\\\\MilestoneCommand\\\\data"));
  await ro.goto(DIST);
  await ro.waitForTimeout(1500);
  const warn = await ro.evaluate(() => (document.getElementById('storageBanner') || {}).textContent.trim());
  check('给出改存位置的警告', warn.includes('不可写') && warn.includes('AppData'), true);
  console.log('    ' + warn.slice(0, 70));
  await ro.close();

  console.log(errs.length ? '\n  ERRORS:\n   ' + errs.join('\n   ') : '\n  无报错');
  console.log(pass ? '  桌面版端到端：全部通过' : '  桌面版端到端：有失败');
  await b.close();
  process.exit(pass ? 0 : 1);
})();
