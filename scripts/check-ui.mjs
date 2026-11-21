// 检查界面那头的两件事：
//
//   1. index.html 里写的每一个 id，app.js 里都要真的用到。
//   2. app.js 和 index.html 里用到的每一个文案键，中文和英文两边都得有。
//
// 跑法： node scripts/check-ui.mjs
// 有问题就以非零的退出码结束。

import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const uiDir = new URL('../crates/skycremote-app/ui/', import.meta.url).pathname;
const backendPath = new URL('../crates/skycremote-app/src/backend.rs', import.meta.url).pathname;

const html = readFileSync(uiDir + 'index.html', 'utf8');
const appJs = readFileSync(uiDir + 'app.js', 'utf8');
const i18nJs = readFileSync(uiDir + 'i18n.js', 'utf8');
const backendJs = readFileSync(backendPath, 'utf8');

let failures = 0;

function check(name, condition, detail = '') {
  if (condition) {
    console.log('  [通过] ' + name);
  } else {
    console.log('  [失败] ' + name + (detail ? ' —— ' + detail : ''));
    failures += 1;
  }
}

/* ------------------------------------------------------------------
 * 把 i18n.js 跑起来，拿到两张表
 * ------------------------------------------------------------------ */

const sandbox = { console };
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(i18nJs, sandbox);

const table = sandbox.SkycUI.i18n.table;
const zhKeys = new Set(Object.keys(table.zh));
const enKeys = new Set(Object.keys(table.en));

/* ------------------------------------------------------------------
 * 1. id
 * ------------------------------------------------------------------ */

console.log('\n== id ==');

const htmlIds = [...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
const usedIds = new Set(
  [...appJs.matchAll(/\$\(\s*['"]#([A-Za-z0-9_-]+)['"]/g)].map((m) => m[1])
);

const uniqueIds = [...new Set(htmlIds)];
const unusedIds = uniqueIds.filter((id) => !usedIds.has(id));
check(
  'index.html 里每个 id 都在 app.js 里用到了',
  unusedIds.length === 0,
  unusedIds.join(', ')
);

const missingInHtml = [...usedIds].filter((id) => !uniqueIds.includes(id));
check(
  'app.js 里用到的 id 都在 index.html 里',
  missingInHtml.length === 0,
  missingInHtml.join(', ')
);

/* ------------------------------------------------------------------
 * 2. 文案键
 * ------------------------------------------------------------------ */

console.log('\n== 文案键 ==');

// t('key') 里的键。键后面直接接 + 的是拼出来的，不算数。
const jsKeys = [...appJs.matchAll(/\bt\(\s*['"]([^'"]+)['"]/g)]
  .map((m) => m[1])
  .filter((key) => !key.endsWith('.'));

// data-i18n / data-i18n-placeholder / data-i18n-title 里的键。
const htmlKeys = [
  ...[...html.matchAll(/data-i18n(?:-placeholder|-title)?="([^"]+)"/g)].map((m) => m[1])
];

const allKeys = [...new Set([...jsKeys, ...htmlKeys])];

const missingZh = allKeys.filter((key) => !zhKeys.has(key));
const missingEn = allKeys.filter((key) => !enKeys.has(key));

check('用到的键在中文表里都有', missingZh.length === 0, missingZh.join(', '));
check('用到的键在英文表里都有', missingEn.length === 0, missingEn.join(', '));

const onlyZh = [...zhKeys].filter((key) => !enKeys.has(key));
const onlyEn = [...enKeys].filter((key) => !zhKeys.has(key));
check('中文表和英文表的键一样多', onlyZh.length === 0 && onlyEn.length === 0,
  [...onlyZh, ...onlyEn].join(', '));

const unusedKeys = [...zhKeys].filter((key) => !allKeys.includes(key));
check('表里没有用不到的键', unusedKeys.length === 0, unusedKeys.join(', '));

console.log('\n  一共 ' + allKeys.length + ' 个键，' + zhKeys.size + ' 个中文，' + enKeys.size + ' 个英文');

/* ------------------------------------------------------------------
 * 3. 命令：前端叫的，后端得认
 * ------------------------------------------------------------------ */

console.log('\n== 命令 ==');

// 后端 dispatch 里那张表。写成一个 match 的样子，所以这里只需要找出
// 那些带引号的命令名。
const backendCommands = new Set(
  [...backendJs.matchAll(/"([a-zA-Z][a-zA-Z0-9]*)"\s*=>/g)].map((m) => m[1])
);

// 前端叫的命令。
const called = [...new Set(
  [...appJs.matchAll(/bridge\.call\(\s*['"]([^'"]+)['"]/g)].map((m) => m[1])
)];

const unknown = called.filter((name) => !backendCommands.has(name));
check(
  '前端叫的命令后端都认',
  unknown.length === 0,
  unknown.join(', ')
);

// 后端认了但前端从来不叫的，是白写在后端的。留几个是给命令行工具
// 用的，不算错。
console.log('  前端一共叫了 ' + called.length + ' 个命令');

/* ------------------------------------------------------------------
 * 4. 不该留的东西
 * ------------------------------------------------------------------ */

console.log('\n== 旧东西 ==');

check('没有 connect/share 的模式切换',
  !/mode-switch|data-mode=|setMode\s*\(/.test(html + appJs));

// 这一版删掉的那些 id，不该再出现在页面里。
const REMOVED_IDS = [
  'view-connect', 'control-hero', 'connect-form', 'connect-hero',
  'share-panel', 'share-card', 'share-form', 'mode-switch', 'device-tabs',
  'file-drawer', 'terminal-panel', 'cloud-keyboard', 'gamepad-panel',
  'keymap-modal', 'confirm-modal', 'context-menu', 'privacy-shield',
  'reconnect-overlay', 'status-banner', 'device-empty-icon', 'footer-license',
  'settings-panel-x'
];
const leftOver = uniqueIds.filter((id) => REMOVED_IDS.includes(id));
check('没有已经删掉的 DOM id', leftOver.length === 0, leftOver.join(', '));

console.log('');
if (failures) {
  console.log('有问题：' + failures + ' 处');
  process.exit(1);
}
console.log('都对上了。');
