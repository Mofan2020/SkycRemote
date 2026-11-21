/*!
 * SkycRemote UI — i18n
 *
 * Flat key/value tables for 中文 and English.  Lookup is a single flat
 * read of TABLE[lang][key]; there is no nested path, so a key can
 * never fall through to a different level than the one it was written
 * on.  Dots inside a key are part of the key name.
 *
 * Loaded as a classic script (not an ES module) so that it works from
 * a file:// URL inside WKWebView / WebView2 / WebKitGTK without CORS
 * problems.  Everything hangs off window.SkycUI.i18n.
 */
(function (global) {
  'use strict';

  var NS = (global.SkycUI = global.SkycUI || {});

  var TABLE = {
    zh: {
      /* ---------- 应用 ---------- */
      'app.name': 'SkycRemote',

      /* ---------- 通用 ---------- */
      'common.close': '关闭',
      'common.save': '保存',
      'common.copyFailed': '复制不了，请手动选中复制',
      'common.refresh': '刷新',
      'common.remove': '删除',

      /* ---------- 顶栏 ---------- */
      'topbar.language': '界面语言',
      'topbar.theme': '换主题',
      'topbar.logs': '日志',
      'topbar.settings': '设置',

      /* ---------- 我的设备 ---------- */
      'home.myDevice': '我的设备',
      'home.myDevice.id': '设备 ID',
      'home.myDevice.name': '显示名称',
      'home.myDevice.tracker': 'Tracker 地址',
      'home.myDevice.trackerHint': '两台电脑要连到同一个 Tracker',
      'home.myDevice.copyId': '复制设备 ID',
      'home.myDevice.share': '共享这台电脑',
      'home.myDevice.stopShare': '停止共享',
      'home.myDevice.shareHint': '把设备 ID 和访问口令告诉对方，对方才能连上',
      'home.myDevice.accessCode': '访问口令',
      'home.myDevice.accessCodeNone': '没有设口令',
      'home.myDevice.accessCodeHint': '对方连进来的时候要输入这个口令',
      'home.myDevice.verifyCode': '安全码',
      'home.myDevice.verifyCodeHint': '让对方核对，两边数字一样才安全',
      'home.share.waiting': '等待连接',
      'home.share.sharing': '正在共享',
      'home.share.warning': '共享出了点问题',
      'home.share.stopped': '没有共享',

      /* ---------- 远程设备 ---------- */
      'home.remote': '远程设备',
      'home.remote.search': '搜索设备',
      'home.remote.searchEmpty': '没有找到这台设备',
      'home.remote.manualPlaceholder': '设备 ID',
      'home.remote.connect': '连接',
      'home.remote.empty': '还没有别的设备',
      'home.remote.emptyHint': '在另一台电脑上打开 SkycRemote，这边就会多出一台设备',
      'home.remote.onlySelf': '列表里只有这台电脑自己',
      'home.remote.online': '在线',
      'home.remote.offline': '离线',
      'home.remote.offlineToast': '这台设备现在不在线',
      'home.remote.self': '这台电脑',

      /* ---------- 访问口令 ---------- */
      'access.title': '输入访问口令',
      'access.device': '设备',
      'access.placeholder': '向对方要这台电脑的访问口令',
      'access.remember': '记住这台设备的口令',
      'access.confirm': '连接',
      'access.cancel': '取消',
      'access.wrong': '口令不对。向对方确认访问口令后再试一次',
      'access.missing': '那台电脑还没有设访问口令。请让对方在「设置」里设一个',
      'access.failed': '连不上这台设备',
      'access.mockHint': '演示模式：对方的口令是 1234',

      /* ---------- 远程画面 ---------- */
      'remote.back': '返回设备列表',
      'remote.placeholder': '还没有画面',
      'remote.connecting': '正在连接',
      'remote.stats.quality': '画质',
      'remote.stats.framerate': '帧率',
      'remote.stats.bitrate': '码率',
      'remote.stats.latency': '延迟',
      'remote.stats.resolution': '分辨率',
      'remote.toolbar.screens': '屏幕',
      'remote.toolbar.fullscreen': '全屏',
      'remote.toolbar.exitFullscreen': '退出全屏',
      'remote.toolbar.disconnect': '断开',
      'remote.toolbar.more': '更多',
      'remote.toolbar.files': '文件传输',
      'remote.toolbar.terminal': '终端',
      'remote.toolbar.cloudKeyboard': '云键盘',
      'remote.toolbar.gamepad': '手柄',
      'remote.toolbar.privacy': '隐私屏',
      'remote.toolbar.virtual': '虚拟屏幕',
      'remote.toolbar.power': '电源',
      'remote.toolbar.mapping': '按键映射',
      'remote.verify': '安全码',
      'remote.verify.hint': '和对方屏幕上显示的数字一致，说明中间没人',
      'remote.notReady': '这个还没接上',
      'remote.notReadyHint': '后端还没有这个功能，做不了',

      /* ---------- 设置 ---------- */
      'settings.title': '设置',
      'settings.group.general': '通用',
      'settings.group.language': '界面语言',
      'settings.group.theme': '主题',
      'settings.theme.system': '跟随系统',
      'settings.theme.light': '浅色',
      'settings.theme.dark': '深色',
      'settings.group.connection': '连接',
      'settings.group.tracker': 'Tracker 地址',
      'settings.group.trackerHint': '两台电脑要连到同一个 Tracker',
      'settings.group.deviceId': '设备 ID',
      'settings.group.deviceIdHint': '这台电脑的编号，对方连你的时候要用',
      'settings.group.deviceName': '显示名称',
      'settings.group.quality': '画质',
      'settings.quality.auto': '自动',
      'settings.quality.4k': '4K',
      'settings.quality.fps144': '144 帧',
      'settings.quality.1080p60': '1080p60',
      'settings.quality.1080p': '1080p',
      'settings.group.bitrate': '码率',
      'settings.group.bitrateHint': '每秒传多少数据，越高越清楚，也越吃带宽',
      'settings.group.framerate': '帧率',
      'settings.group.framerateHint': '每秒传多少张画面',
      'settings.group.security': '安全',
      'settings.group.accessCode': '本机访问口令',
      'settings.group.accessCodeHint': '别人连你的时候要输入这个口令。留空表示不用口令',
      'settings.group.savedCodes': '我已记住的设备口令',
      'settings.group.savedCodesEmpty': '还没有记住任何口令',
      'settings.group.savedCodesHint': '这些口令只存在这台电脑上',
      'settings.group.about': '关于',
      'settings.about.version': '版本',
      'settings.readOnly': '设备 ID 和显示名称在这里改，别的地方只显示',
      'settings.quit': '退出 SkycRemote',

      /* ---------- 日志 ---------- */
      'log.title': '日志',
      'log.clear': '清空',
      'log.empty': '还没有日志',
      'log.count': '{n} 条',

      /* ---------- 提示 ---------- */
      'toast.connected': '已连接到 {name}',
      'toast.disconnected': '已断开',
      'toast.settingsSaved': '设置已保存',
      'toast.copied': '已复制到剪贴板',
      'toast.mockNotice': '演示模式：后端还没接上，这里的数据都是假的',
      'toast.hostStarted': '这台电脑开始共享了',
      'toast.hostStopped': '共享已停止',
      'toast.hostWarning': '共享出了点问题',
      'toast.logCleared': '日志已清空',
      'toast.codeSaved': '口令已记住',
      'toast.screenSwitched': '已切到 {name}',
      'toast.quit': '程序退出了'
    },

    en: {
      /* ---------- app ---------- */
      'app.name': 'SkycRemote',

      /* ---------- common ---------- */
      'common.close': 'Close',
      'common.save': 'Save',
      'common.copyFailed': 'Could not copy it; select the text and copy it yourself',
      'common.refresh': 'Refresh',
      'common.remove': 'Remove',

      /* ---------- top bar ---------- */
      'topbar.language': 'Language',
      'topbar.theme': 'Change the theme',
      'topbar.logs': 'Log',
      'topbar.settings': 'Settings',

      /* ---------- this computer ---------- */
      'home.myDevice': 'This computer',
      'home.myDevice.id': 'Device ID',
      'home.myDevice.name': 'Display name',
      'home.myDevice.tracker': 'Tracker address',
      'home.myDevice.trackerHint': 'Both computers need the same tracker',
      'home.myDevice.copyId': 'Copy device ID',
      'home.myDevice.share': 'Share this computer',
      'home.myDevice.stopShare': 'Stop sharing',
      'home.myDevice.shareHint': 'Give the other side your device ID and access code',
      'home.myDevice.accessCode': 'Access code',
      'home.myDevice.accessCodeNone': 'No access code set',
      'home.myDevice.accessCodeHint': 'The other side types this code to connect',
      'home.myDevice.verifyCode': 'Safety code',
      'home.myDevice.verifyCodeHint': 'Have the other side check it; the numbers must match',
      'home.share.waiting': 'Waiting',
      'home.share.sharing': 'Sharing',
      'home.share.warning': 'Something is wrong',
      'home.share.stopped': 'Not sharing',

      /* ---------- other devices ---------- */
      'home.remote': 'Other devices',
      'home.remote.search': 'Search devices',
      'home.remote.searchEmpty': 'No device found',
      'home.remote.manualPlaceholder': 'Device ID',
      'home.remote.connect': 'Connect',
      'home.remote.empty': 'No other devices yet',
      'home.remote.emptyHint': 'Open SkycRemote on the other computer and it shows up here',
      'home.remote.onlySelf': 'The only device here is this one',
      'home.remote.online': 'Online',
      'home.remote.offline': 'Offline',
      'home.remote.offlineToast': 'That device is not online right now',
      'home.remote.self': 'This computer',

      /* ---------- access code ---------- */
      'access.title': 'Enter the access code',
      'access.device': 'Device',
      'access.placeholder': 'Ask the other side for this computer\u2019s access code',
      'access.remember': 'Remember the code for this device',
      'access.confirm': 'Connect',
      'access.cancel': 'Cancel',
      'access.wrong': 'That code is not right. Check the access code with the other side and try again',
      'access.missing': 'That computer has no access code yet. Ask them to set one in Settings',
      'access.failed': 'Could not connect to that device',
      'access.mockHint': 'Demo mode: the other side\u2019s code is 1234',

      /* ---------- the remote picture ---------- */
      'remote.back': 'Back to the device list',
      'remote.placeholder': 'No picture yet',
      'remote.connecting': 'Connecting',
      'remote.stats.quality': 'Quality',
      'remote.stats.framerate': 'Frame rate',
      'remote.stats.bitrate': 'Bitrate',
      'remote.stats.latency': 'Latency',
      'remote.stats.resolution': 'Resolution',
      'remote.toolbar.screens': 'Screens',
      'remote.toolbar.fullscreen': 'Full screen',
      'remote.toolbar.exitFullscreen': 'Leave full screen',
      'remote.toolbar.disconnect': 'Disconnect',
      'remote.toolbar.more': 'More',
      'remote.toolbar.files': 'File transfer',
      'remote.toolbar.terminal': 'Terminal',
      'remote.toolbar.cloudKeyboard': 'On-screen keyboard',
      'remote.toolbar.gamepad': 'Gamepad',
      'remote.toolbar.privacy': 'Privacy screen',
      'remote.toolbar.virtual': 'Virtual screen',
      'remote.toolbar.power': 'Power',
      'remote.toolbar.mapping': 'Key mapping',
      'remote.verify': 'Safety code',
      'remote.verify.hint': 'The same number on their screen means nobody is in between',
      'remote.notReady': 'This one is not wired up yet',
      'remote.notReadyHint': 'The backend does not have this yet',

      /* ---------- settings ---------- */
      'settings.title': 'Settings',
      'settings.group.general': 'General',
      'settings.group.language': 'Language',
      'settings.group.theme': 'Theme',
      'settings.theme.system': 'Match the system',
      'settings.theme.light': 'Light',
      'settings.theme.dark': 'Dark',
      'settings.group.connection': 'Connection',
      'settings.group.tracker': 'Tracker address',
      'settings.group.trackerHint': 'Both computers need the same tracker',
      'settings.group.deviceId': 'Device ID',
      'settings.group.deviceIdHint': 'The number of this computer; the other side needs it to connect',
      'settings.group.deviceName': 'Display name',
      'settings.group.quality': 'Quality',
      'settings.quality.auto': 'Auto',
      'settings.quality.4k': '4K',
      'settings.quality.fps144': '144 fps',
      'settings.quality.1080p60': '1080p60',
      'settings.quality.1080p': '1080p',
      'settings.group.bitrate': 'Bitrate',
      'settings.group.bitrateHint': 'How much data goes out each second; more is clearer and uses more bandwidth',
      'settings.group.framerate': 'Frame rate',
      'settings.group.framerateHint': 'How many pictures go out each second',
      'settings.group.security': 'Security',
      'settings.group.accessCode': "This computer's access code",
      'settings.group.accessCodeHint': 'The other side types this to connect. Leave it empty for no code',
      'settings.group.savedCodes': 'Access codes I remember',
      'settings.group.savedCodesEmpty': 'No codes remembered yet',
      'settings.group.savedCodesHint': 'These codes stay on this computer',
      'settings.group.about': 'About',
      'settings.about.version': 'Version',
      'settings.readOnly': 'The device ID and display name are changed here; everywhere else they are only shown',
      'settings.quit': 'Quit SkycRemote',

      /* ---------- log ---------- */
      'log.title': 'Log',
      'log.clear': 'Clear',
      'log.empty': 'Nothing logged yet',
      'log.count': '{n} entries',

      /* ---------- notices ---------- */
      'toast.connected': 'Connected to {name}',
      'toast.disconnected': 'Disconnected',
      'toast.settingsSaved': 'Settings saved',
      'toast.copied': 'Copied to the clipboard',
      'toast.mockNotice': 'Demo mode: the backend is not wired up yet, so all the data here is made up',
      'toast.hostStarted': 'This computer is sharing now',
      'toast.hostStopped': 'Sharing stopped',
      'toast.hostWarning': 'Something went wrong with the share',
      'toast.logCleared': 'Log cleared',
      'toast.codeSaved': 'The code is remembered',
      'toast.screenSwitched': 'Switched to {name}',
      'toast.quit': 'The program quit'
    }
  };

  var SUPPORTED = ['zh', 'en'];
  var current = 'zh';

  function detect() {
    try {
      var stored = global.localStorage && global.localStorage.getItem('skycremote.locale');
      if (stored && SUPPORTED.indexOf(stored) !== -1) return stored;
    } catch (e) { /* private mode, ignore */ }
    var nav = (global.navigator && (global.navigator.language || global.navigator.userLanguage)) || 'zh';
    return /^zh/i.test(nav) ? 'zh' : 'en';
  }

  /** Replace {name} placeholders with the given values. */
  function interpolate(template, vars) {
    if (!vars) return template;
    return String(template).replace(/\{(\w+)\}/g, function (whole, key) {
      return Object.prototype.hasOwnProperty.call(vars, key) ? String(vars[key]) : whole;
    });
  }

  /**
   * Translate a key.  Falls back to English, then to the key itself,
   * so a missing entry is visible instead of silently blank.
   */
  function t(key, vars) {
    var table = TABLE[current] || TABLE.en;
    var value = table[key];
    if (value === undefined) value = TABLE.en[key];
    if (value === undefined) return key;
    return interpolate(value, vars);
  }

  function setLocale(lang) {
    if (SUPPORTED.indexOf(lang) === -1) return current;
    current = lang;
    try {
      if (global.localStorage) global.localStorage.setItem('skycremote.locale', lang);
    } catch (e) { /* ignore */ }
    if (global.document && global.document.documentElement) {
      global.document.documentElement.setAttribute('lang', lang === 'zh' ? 'zh-CN' : 'en');
    }
    return current;
  }

  function getLocale() { return current; }

  /** Keys present in one language but missing in the other. */
  function audit() {
    var zh = Object.keys(TABLE.zh);
    var en = Object.keys(TABLE.en);
    var missingInEn = zh.filter(function (k) { return en.indexOf(k) === -1; });
    var missingInZh = en.filter(function (k) { return zh.indexOf(k) === -1; });
    return {
      total: zh.length,
      missingInEn: missingInEn,
      missingInZh: missingInZh,
      ok: missingInEn.length === 0 && missingInZh.length === 0
    };
  }

  NS.i18n = {
    t: t,
    setLocale: setLocale,
    getLocale: getLocale,
    detect: detect,
    audit: audit,
    supported: SUPPORTED.slice(),
    table: TABLE
  };
})(typeof window !== 'undefined' ? window : globalThis);
