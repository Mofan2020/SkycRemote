/*!
 * SkycRemote UI — application
 *
 * Plain HTML/CSS/JS. No framework, no build step. Loaded as a classic
 * script so it runs from a file:// URL inside WKWebView / WebView2 /
 * WebKitGTK without a CORS dance.
 *
 * The only global this file creates is `window.skycremote`, the API the
 * Rust side calls into. Everything else lives in this closure.
 *
 * ---------------------------------------------------------------------
 * The backend is a mock.  See components/bridge.js for the seam where
 * the real Rust transport plugs in, and for the list of calls that are
 * currently answered with fake data.
 * ---------------------------------------------------------------------
 */
(function (global) {
  'use strict';

  var NS = global.SkycUI;
  var icons = NS.icons;
  var i18n = NS.i18n;
  var bridge = NS.bridge;

  var t = i18n.t;

  /* ==================================================================
   * State
   * ================================================================== */

  var DEFAULT_SETTINGS = {
    quality: 'auto',
    bitrate: 50,
    framerate: 60,
    encoder: 'auto',
    audioEnabled: true,
    audioQuality: 'high',
    keymapPreset: 'default',
    keymapRows: [],
    gamepadEnabled: true,
    privacyBlackout: true,
    privacyAutoLock: false,
    screensaver: 'none',
    privacyScreen: false,
    fileDownloadDir: '~/Downloads/SkycRemote',
    terminalEnabled: true,
    terminalShell: 'default',
    portMappings: [],
    language: 'zh',
    theme: 'system'
  };

  var state = {
    view: 'connect',
    // The decoder for the pictures.  It is made when the first
    // picture arrives, because a browser that cannot decode H.264
    // should say so once rather than on every picture.
    decoder: null,
    themePref: 'system',
    resolvedTheme: 'light',
    settings: clone(DEFAULT_SETTINGS),
    devices: [],
    deviceTab: 'recent',
    deviceQuery: '',
    screenIndex: 0,
    screens: [],
    virtualScreen: false,
    privacyScreen: false,
    toolbarVisible: true,
    toolbarPinned: false,
    cursorStyle: 'default',
    connected: false,
    connecting: false,
    reconnecting: false,
    connectionStatus: 'idle',
    latency: null,
    bitrate: null,
    framerate: null,
    resolution: null,
    currentPath: '/',
    fileQueue: [],
    terminalLines: [],
    terminalOpen: false,
    fileOpen: false,
    settingsOpen: false,
    keymapRecording: null,
    gamepadPollTimer: null,
    lastRemoteSize: { width: 0, height: 0 }
  };

  function clone(obj) {
    return JSON.parse(JSON.stringify(obj));
  }

  /* ==================================================================
   * Small helpers
   * ================================================================== */

  function $(sel, root) { return (root || document).querySelector(sel); }
  function $$(sel, root) { return Array.prototype.slice.call((root || document).querySelectorAll(sel)); }

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = String(text);
    return node;
  }

  function show(node) { if (node) node.hidden = false; }
  function hide(node) { if (node) node.hidden = true; }
  function toggle(node, on) { if (node) node.hidden = !on; }

  function setHidden(node, hidden) { if (node) node.hidden = !!hidden; }

  function formatBytes(bytes) {
    if (bytes === null || bytes === undefined) return '—';
    if (bytes < 1024) return Math.round(bytes) + ' ' + t('common.bytes');
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' ' + t('common.kb');
    if (bytes < 1024 * 1024 * 1024) return (bytes / 1048576).toFixed(1) + ' ' + t('common.mb');
    return (bytes / 1073741824).toFixed(2) + ' ' + t('common.gb');
  }

  /** Round a byte count to a readable magnitude before showing it. */
  function formatRate(bytesPerSecond) {
    if (!isFinite(bytesPerSecond) || bytesPerSecond <= 0) return '0 ' + t('common.bytes');
    if (bytesPerSecond < 1024) return Math.round(bytesPerSecond) + ' ' + t('common.bytes');
    if (bytesPerSecond < 1048576) return (bytesPerSecond / 1024).toFixed(1) + ' ' + t('common.kb');
    return (bytesPerSecond / 1048576).toFixed(1) + ' ' + t('common.mb');
  }

  function formatEta(seconds) {
    if (!isFinite(seconds) || seconds < 0) return '—';
    if (seconds < 60) return Math.round(seconds) + 's';
    if (seconds < 3600) return Math.floor(seconds / 60) + 'm ' + Math.round(seconds % 60) + 's';
    return Math.floor(seconds / 3600) + 'h ' + Math.floor((seconds % 3600) / 60) + 'm';
  }

  function formatTime(iso) {
    try {
      var d = new Date(iso);
      var diff = (Date.now() - d.getTime()) / 1000;
      if (diff < 60) return 'just now';
      if (diff < 3600) return Math.floor(diff / 60) + 'm ago';
      if (diff < 86400) return Math.floor(diff / 3600) + 'h ago';
      if (diff < 86400 * 7) return Math.floor(diff / 86400) + 'd ago';
      return d.toLocaleDateString();
    } catch (e) {
      return '—';
    }
  }

  function clamp(v, lo, hi) { return Math.min(hi, Math.max(lo, v)); }

  /**
   * Normalise a list of screens coming from the backend.  Rust's serde
   * writes `scale_factor`; the UI reads `scaleFactor`.  The bridge
   * handles both, and this keeps the event path consistent with it.
   */
  function normaliseScreens(list) {
    if (!Array.isArray(list)) return [];
    return list.map(function (s) {
      return bridge.normaliseScreen ? bridge.normaliseScreen(s) : s;
    }).filter(Boolean);
  }

  function debounce(fn, ms) {
    var timer = null;
    return function () {
      var args = arguments;
      var self = this;
      clearTimeout(timer);
      timer = setTimeout(function () { fn.apply(self, args); }, ms);
    };
  }

  /* ==================================================================
   * i18n application
   * ================================================================== */

  function applyI18n(root) {
    var scope = root || document;
    $$('[data-i18n]', scope).forEach(function (node) {
      node.textContent = t(node.getAttribute('data-i18n'));
    });
    $$('[data-i18n-placeholder]', scope).forEach(function (node) {
      node.setAttribute('placeholder', t(node.getAttribute('data-i18n-placeholder')));
    });
    $$('[data-i18n-title]', scope).forEach(function (node) {
      node.setAttribute('title', t(node.getAttribute('data-i18n-title')));
    });
  }

  /* ==================================================================
   * Icons
   * ================================================================== */

  /** Replace every <span data-icon="name"> with its SVG. */
  function hydrateIcons(root) {
    $$('[data-icon]', root || document).forEach(function (node) {
      var name = node.getAttribute('data-icon');
      if (!name) return;
      node.innerHTML = icons.icon(name, { size: parseInt(node.getAttribute('data-icon-size'), 10) || 18 });
      node.classList.add('icon-filled');
    });
  }

  /* ==================================================================
   * Theme
   * ================================================================== */

  var systemDark = global.matchMedia ? global.matchMedia('(prefers-color-scheme: dark)') : null;

  function resolveTheme(pref) {
    if (pref === 'light' || pref === 'dark') return pref;
    if (systemDark) return systemDark.matches ? 'dark' : 'light';
    return 'light';
  }

  function applyTheme(pref, persist) {
    state.themePref = pref;
    state.resolvedTheme = resolveTheme(pref);
    document.documentElement.setAttribute('data-theme', state.resolvedTheme);
    var themeIcon = $('#theme-icon');
    if (themeIcon) {
      var wanted = state.resolvedTheme === 'dark' ? 'moon' : 'sun';
      if (themeIcon.getAttribute('data-icon') !== wanted) {
        themeIcon.setAttribute('data-icon', wanted);
        hydrateIcons(themeIcon.parentNode);
      }
    }
    if (persist) {
      state.settings.theme = pref;
      try { global.localStorage.setItem('skycremote.theme', pref); } catch (e) { /* ignore */ }
      var sel = $('#settings-theme');
      if (sel) sel.value = pref;
    }
  }

  /* ==================================================================
   * Toasts
   * ================================================================== */

  var TOAST_ICON = { success: 'check-circle', warning: 'alert-triangle', error: 'x-circle', info: 'info' };

  function toast(message, kind, duration) {
    var host = $('#toast-container');
    if (!host) return;
    kind = kind || 'info';

    var node = el('div', 'toast toast-' + kind);
    var ic = el('span', 'icon toast-icon');
    ic.innerHTML = icons.icon(TOAST_ICON[kind] || 'info', { size: 18 });
    node.appendChild(ic);
    node.appendChild(el('span', 'toast-msg', message));

    var close = el('button', 'icon-btn icon-btn-xs toast-close');
    close.innerHTML = icons.icon('x', { size: 14 });
    close.type = 'button';
    node.appendChild(close);

    host.appendChild(node);
    requestAnimationFrame(function () { node.classList.add('is-in'); });

    var timer = setTimeout(function () { dismiss(node); }, duration || (kind === 'error' ? 5200 : 3200));
    close.addEventListener('click', function () { clearTimeout(timer); dismiss(node); });
    return node;
  }

  function dismiss(node) {
    if (!node || !node.parentNode) return;
    node.classList.remove('is-in');
    node.classList.add('is-out');
    setTimeout(function () { if (node.parentNode) node.parentNode.removeChild(node); }, 220);
  }

  /* ==================================================================
   * Menus (click-outside close)
   * ================================================================== */

  var openMenu = null;

  function showMenu(menu, trigger) {
    if (openMenu && openMenu.menu) hide(openMenu.menu);
    show(menu);
    if (trigger) trigger.setAttribute('aria-expanded', 'true');
    openMenu = { menu: menu, trigger: trigger };
  }

  function closeMenus() {
    if (!openMenu) return;
    hide(openMenu.menu);
    if (openMenu.trigger) openMenu.trigger.setAttribute('aria-expanded', 'false');
    openMenu = null;
  }

  document.addEventListener('pointerdown', function (e) {
    if (!openMenu) return;
    if (openMenu.menu.contains(e.target)) return;
    if (openMenu.trigger && openMenu.trigger.contains(e.target)) return;
    closeMenus();
  });

  /* The context menu is a separate surface from the dropdown menus; it
     dismisses on any click that is not on one of its own items. */
  document.addEventListener('click', function (e) {
    var ctx = $('#context-menu');
    if (!ctx || ctx.hidden) return;
    if (ctx.contains(e.target)) return;
    ctx.hidden = true;
    ctx.classList.remove('is-in');
  });

  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') {
      if (openMenu) { closeMenus(); return; }
      closeOverlays();
    }
  });

  /* ==================================================================
   * Modal helpers
   * ================================================================== */

  var confirmResolver = null;

  function confirmDialog(message) {
    var modal = $('#confirm-modal');
    var msg = $('#confirm-message');
    if (msg) msg.textContent = message;
    show(modal);
    return new Promise(function (resolve) { confirmResolver = resolve; });
  }

  function wireConfirm() {
    var ok = $('#confirm-ok');
    var cancel = $('#confirm-cancel');
    if (ok) ok.addEventListener('click', function () {
      hide($('#confirm-modal'));
      if (confirmResolver) { confirmResolver(true); confirmResolver = null; }
    });
    if (cancel) cancel.addEventListener('click', function () {
      hide($('#confirm-modal'));
      if (confirmResolver) { confirmResolver(false); confirmResolver = null; }
    });
  }

  /* ==================================================================
   * Settings
   * ================================================================== */

  function loadSettings() {
    var stored = null;
    try { stored = global.localStorage.getItem('skycremote.settings'); } catch (e) { /* ignore */ }
    if (stored) {
      try {
        var parsed = JSON.parse(stored);
        Object.keys(DEFAULT_SETTINGS).forEach(function (key) {
          if (Object.prototype.hasOwnProperty.call(parsed, key)) state.settings[key] = parsed[key];
        });
      } catch (e) { /* corrupt, keep defaults */ }
    }
    applyTheme(state.settings.theme || 'system', false);
    i18n.setLocale(state.settings.language || i18n.detect());
  }

  function persistSettings() {
    try {
      global.localStorage.setItem('skycremote.settings', JSON.stringify(state.settings));
    } catch (e) { /* ignore */ }
  }

  /** Read every control in the settings panel into `state.settings`. */
  function readSettingsForm() {
    var s = state.settings;
    s.quality = $('#quality-select').value;
    s.bitrate = parseInt($('#bitrate-slider').value, 10) || 50;
    s.framerate = parseInt($('#framerate-slider').value, 10) || 60;
    s.encoder = $('#encoder-select').value;
    s.audioEnabled = $('#audio-enabled').checked;
    s.audioQuality = $('#audio-quality').value;
    s.keymapPreset = $('#keymap-select').value;
    s.gamepadEnabled = $('#gamepad-enabled-setting').checked;
    s.privacyBlackout = $('#privacy-blackout').checked;
    s.privacyAutoLock = $('#privacy-autolock').checked;
    s.screensaver = $('#screensaver-select').value;
    s.privacyScreen = $('#privacy-screen-toggle').checked;
    s.fileDownloadDir = $('#file-dir-input').value;
    s.terminalEnabled = $('#terminal-enabled').checked;
    s.terminalShell = $('#terminal-shell').value;
    s.language = $('#settings-language').value;
    s.theme = $('#settings-theme').value;
  }

  /** Push `state.settings` back into the settings form. */
  function writeSettingsForm() {
    var s = state.settings;
    $('#quality-select').value = s.quality;
    $('#bitrate-slider').value = s.bitrate;
    $('#bitrate-value').textContent = s.bitrate + ' Mbps';
    $('#framerate-slider').value = s.framerate;
    $('#framerate-value').textContent = s.framerate + ' FPS';
    $('#encoder-select').value = s.encoder;
    $('#audio-enabled').checked = !!s.audioEnabled;
    $('#audio-quality').value = s.audioQuality;
    $('#keymap-select').value = s.keymapPreset;
    $('#gamepad-enabled-setting').checked = !!s.gamepadEnabled;
    $('#privacy-blackout').checked = !!s.privacyBlackout;
    $('#privacy-autolock').checked = !!s.privacyAutoLock;
    $('#screensaver-select').value = s.screensaver;
    $('#privacy-screen-toggle').checked = !!s.privacyScreen;
    $('#file-dir-input').value = s.fileDownloadDir;
    $('#terminal-enabled').checked = !!s.terminalEnabled;
    $('#terminal-shell').value = s.terminalShell;
    $('#settings-language').value = s.language;
    $('#settings-theme').value = s.theme;
    renderPorts();
  }

  function openSettings() {
    writeSettingsForm();
    hide($('#context-menu'));
    show($('#settings-panel'));
    state.settingsOpen = true;
    syncDrawerClass();
  }

  function closeSettings() {
    hide($('#settings-panel'));
    state.settingsOpen = false;
    syncDrawerClass();
  }

  function saveSettings() {
    readSettingsForm();
    persistSettings();
    applyTheme(state.settings.theme, false);
    i18n.setLocale(state.settings.language);
    applyI18n();
    refreshQualityDisplay();
    syncToolbarDensity();
    toast(t('toast.settingsSaved'), 'success');
  }

  function resetSettings() {
    var lang = state.settings.language;
    state.settings = clone(DEFAULT_SETTINGS);
    state.settings.language = lang;
    writeSettingsForm();
    persistSettings();
    applyTheme(state.settings.theme, false);
    applyI18n();
    toast(t('toast.settingsReset'), 'success');
  }

  /* ---- port mapping rows ---- */

  function renderPorts() {
    var host = $('#port-list');
    if (!host) return;
    host.innerHTML = '';
    var mappings = state.settings.portMappings || [];
    var empty = $('#port-empty');
    if (empty) empty.hidden = mappings.length > 0;

    mappings.forEach(function (m, index) {
      var row = el('div', 'port-row');
      row.appendChild(numberField(m.localPort, 'local', index));
      row.appendChild(el('span', 'port-arrow', '→'));
      row.appendChild(numberField(m.remotePort, 'remote', index));
      row.appendChild(textField(m.remoteHost, 'host', index));
      var del = el('button', 'icon-btn icon-btn-sm port-del');
      del.type = 'button';
      del.innerHTML = icons.icon('trash', { size: 15 });
      del.addEventListener('click', function () {
        state.settings.portMappings.splice(index, 1);
        renderPorts();
      });
      row.appendChild(del);
      host.appendChild(row);
    });
  }

  function numberField(value, kind, index) {
    var input = el('input', 'input input-sm');
    input.type = 'number';
    input.min = '1';
    input.max = '65535';
    input.value = value == null ? '' : value;
    input.setAttribute('aria-label', kind);
    input.addEventListener('input', function () {
      var m = state.settings.portMappings[index];
      if (!m) return;
      if (kind === 'local') m.localPort = parseInt(input.value, 10) || 0;
      if (kind === 'remote') m.remotePort = parseInt(input.value, 10) || 0;
    });
    return input;
  }

  function textField(value, kind, index) {
    var input = el('input', 'input input-sm');
    input.type = 'text';
    input.value = value == null ? '' : value;
    input.setAttribute('aria-label', kind);
    input.addEventListener('input', function () {
      var m = state.settings.portMappings[index];
      if (!m) return;
      if (kind === 'host') m.remoteHost = input.value;
    });
    return input;
  }

  /* ==================================================================
   * Connect view
   * ================================================================== */

  function renderDevices() {
    var list = $('#device-list');
    var empty = $('#device-empty');
    if (!list) return;

    var query = state.deviceQuery.trim().toLowerCase();
    var devices = state.devices.filter(function (d) {
      if (state.deviceTab === 'online' && !d.online) return false;
      if (state.deviceTab === 'favorite' && !d.favorite) return false;
      if (query) {
        var hay = (d.name + ' ' + d.id + ' ' + d.platform).toLowerCase();
        if (hay.indexOf(query) === -1) return false;
      }
      return true;
    });

    list.innerHTML = '';

    if (!devices.length) {
      show(empty);
      var title = $('#device-empty-title');
      var hint = $('#device-empty-hint');
      if (title) {
        title.textContent = query ? t('connect.search.empty')
          : state.deviceTab === 'favorite' ? t('connect.empty.favorite')
          : state.deviceTab === 'online' ? t('connect.empty.online')
          : t('connect.empty.recent');
      }
      if (hint) hint.textContent = query ? '' : t('connect.empty.hint');
      return;
    }
    hide(empty);

    devices.forEach(function (device) {
      list.appendChild(deviceRow(device));
    });
  }

  function deviceRow(device) {
    var row = el('div', 'device-row');
    row.setAttribute('role', 'listitem');

    var avatar = el('div', 'device-avatar');
    avatar.appendChild(platformIcon(device.platform));

    var main = el('div', 'device-main');
    var nameLine = el('div', 'device-name-line');
    nameLine.appendChild(el('span', 'device-name', device.name));

    var status = el('span', 'device-status' + (device.online ? ' is-online' : ''));
    status.appendChild(el('i', 'status-dot'));
    status.appendChild(el('span', '', device.online ? t('connect.device.online') : t('connect.device.offline')));
    nameLine.appendChild(status);

    main.appendChild(nameLine);

    var meta = el('div', 'device-meta');
    meta.appendChild(el('span', 'device-id', device.id));
    meta.appendChild(el('span', 'meta-sep', '·'));
    meta.appendChild(el('span', 'device-platform', device.platform));
    if (device.online && device.latency != null) {
      meta.appendChild(el('span', 'meta-sep', '·'));
      meta.appendChild(el('span', 'device-latency', t('connect.device.latency', { ms: device.latency })));
    } else if (device.lastSeen) {
      meta.appendChild(el('span', 'meta-sep', '·'));
      meta.appendChild(el('span', 'device-seen', t('connect.device.lastSeen', { time: formatTime(device.lastSeen) })));
    }
    main.appendChild(meta);

    row.appendChild(avatar);
    row.appendChild(main);

    var actions = el('div', 'device-actions');

    var fav = el('button', 'icon-btn icon-btn-sm' + (device.favorite ? ' is-fav' : ''));
    fav.type = 'button';
    fav.innerHTML = icons.icon('star', { size: 16 });
    fav.setAttribute('title', device.favorite ? t('connect.device.unfavorite') : t('connect.device.favorite'));
    fav.addEventListener('click', function (e) {
      e.stopPropagation();
      device.favorite = !device.favorite;
      fav.classList.toggle('is-fav', device.favorite);
      fav.innerHTML = icons.icon('star', { size: 16 });
      renderDevices();
    });
    actions.appendChild(fav);

    var connect = el('button', 'btn btn-secondary btn-sm device-connect');
    connect.type = 'button';
    connect.textContent = t('connect.device.connect');
    connect.disabled = !device.online;
    if (!device.online) connect.classList.add('is-disabled');
    connect.addEventListener('click', function (e) {
      e.stopPropagation();
      startConnect(device.id);
    });
    actions.appendChild(connect);

    row.appendChild(actions);

    row.addEventListener('click', function () {
      if (device.online) startConnect(device.id);
    });
    row.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && device.online) startConnect(device.id);
    });
    row.tabIndex = 0;
    return row;
  }

  function platformIcon(platform) {
    var name = 'monitor';
    var p = (platform || '').toLowerCase();
    if (p.indexOf('windows') !== -1) name = 'display';
    else if (p.indexOf('linux') !== -1) name = 'server';
    else if (p.indexOf('mac') !== -1 || p.indexOf('os x') !== -1) name = 'laptop';
    else if (p.indexOf('android') !== -1 || p.indexOf('ios') !== -1) name = 'smartphone';
    var span = el('span', 'icon');
    span.innerHTML = icons.icon(name, { size: 20 });
    return span;
  }

  function loadDevices() {
    // MOCK: real device list comes from the tracker + local history.
    return bridge.call('listDevices').then(function (devices) {
      state.devices = devices || [];
      renderDevices();
    }).catch(function () {
      state.devices = [];
      renderDevices();
    });
  }

  function startConnect(deviceId) {
    if (state.connecting) {
      toast(t('connect.error.busy'), 'warning');
      return;
    }
    var id = (deviceId || ($('#device-id-input') ? $('#device-id-input').value : '') || '').trim();
    if (!id) {
      toast(t('connect.error.empty'), 'warning');
      return;
    }
    if (!/^[A-Za-z0-9-]{4,64}$/.test(id)) {
      toast(t('connect.error.invalid'), 'warning');
      return;
    }

    state.connecting = true;
    setConnectButtonBusy(true);
    setConnectionStatus('punching');

    // MOCK: the real flow is tracker lookup -> NAT hole punch -> handshake.
    bridge.call('connect', { deviceId: id }).then(function () {
      state.connecting = false;
      setConnectButtonBusy(false);
      enterRemoteView(id);
    }).catch(function () {
      state.connecting = false;
      setConnectButtonBusy(false);
      setConnectionStatus('failed');
      toast(t('connect.error.timeout'), 'error');
    });
  }

  function setConnectButtonBusy(busy) {
    var btn = $('#connect-btn');
    var label = $('#connect-btn-label');
    if (btn) btn.classList.toggle('is-busy', busy);
    if (label) label.textContent = busy ? t('connect.connecting') : t('connect.button');
  }

  /* ==================================================================
   * Views
   * ================================================================== */

  function enterRemoteView(deviceId) {
    state.view = 'remote';
    document.getElementById('app').setAttribute('data-view', 'remote');
    var connectView = $('#view-connect');
    var remoteView = $('#view-remote');
    if (connectView) connectView.classList.remove('is-active');
    if (remoteView) remoteView.classList.add('is-active');
    hide($('#connect-hint'));

    state.connected = true;
    setConnectionStatus('connected');
    hide($('#stage-placeholder'));
    var device = findDevice(deviceId);
    toast(t('toast.connected', { name: device ? device.name : deviceId }), 'success');

    // MOCK: fake frames until the real decoder is wired in.
    var canvas = $('#remote-canvas');
    if (canvas && !bridge.isNative()) bridge.mock.startFrames(canvas);

    bridge.call('listScreens').then(function (screens) {
      state.screens = screens || [];
      renderScreenMenu();
      applyScreen(0);
    });

    startStatusPoll();
    var stage = $('#remote-stage');
    if (stage) stage.focus();
  }

  function leaveRemoteView() {
    state.view = 'connect';
    document.getElementById('app').setAttribute('data-view', 'connect');
    var connectView = $('#view-connect');
    var remoteView = $('#view-remote');
    if (connectView) connectView.classList.add('is-active');
    if (remoteView) remoteView.classList.remove('is-active');
    show($('#connect-hint'));
    show($('#stage-placeholder'));

    state.connected = false;
    stopStatusPoll();
    bridge.mock.stopFrames();
    setConnectionStatus('idle');
    renderDevices();
  }

  function findDevice(id) {
    for (var i = 0; i < state.devices.length; i++) {
      if (state.devices[i].id === id) return state.devices[i];
    }
    return null;
  }

  /* ==================================================================
   * Connection status
   * ================================================================== */

  function setConnectionStatus(status) {
    state.connectionStatus = status;
    var banner = $('#status-banner');
    var text = $('#status-text');
    var dot = $('#status-dot');
    var overlay = $('#reconnect-overlay');
    if (!banner) return;

    var labels = {
      idle: t('status.idle'),
      punching: t('status.punching'),
      handshaking: t('status.handshaking'),
      connected: t('status.connected'),
      reconnecting: t('status.reconnecting'),
      closed: t('status.closed'),
      failed: t('status.failed')
    };

    if (status === 'connected') {
      hide(banner);
      hide(overlay);
      if (dot) dot.className = 'status-dot is-ok';
    } else if (status === 'idle' || status === 'closed') {
      hide(banner);
      hide(overlay);
    } else {
      show(banner);
      if (text) text.textContent = labels[status] || status;
      if (dot) dot.className = 'status-dot is-' + (status === 'failed' ? 'err' : 'busy');
      if (status === 'reconnecting') {
        show(overlay);
        var rt = $('#reconnect-text');
        if (rt) rt.textContent = t('status.reconnecting');
      } else {
        hide(overlay);
      }
    }
  }

  var statusTimer = null;

  function startStatusPoll() {
    stopStatusPoll();
    // MOCK: a real backend pushes these numbers with every frame.
    statusTimer = setInterval(function () {
      if (!state.connected) return;
      var jitter = Math.round((Math.random() - 0.5) * 6);
      state.latency = clamp((state.latency || 18) + jitter, 4, 400);
      state.bitrate = Math.max(0.5, (state.bitrate || 8.4) + (Math.random() - 0.5) * 0.6);
      state.framerate = clamp(Math.round((state.framerate || 60) + (Math.random() - 0.5) * 2), 1, state.settings.framerate || 60);
      paintStatus();
    }, 1000);
  }

  function stopStatusPoll() {
    if (statusTimer) { clearInterval(statusTimer); statusTimer = null; }
  }

  function paintStatus() {
    var q = $('#status-quality');
    var f = $('#status-framerate');
    var b = $('#status-bitrate');
    var l = $('#status-latency');
    var r = $('#status-resolution');

    if (q) q.textContent = t('status.quality.' + state.settings.quality) || state.settings.quality;
    if (f) f.textContent = t('status.framerate', { fps: state.framerate || state.settings.framerate });
    if (b) b.textContent = t('status.bitrate', { mbps: (state.bitrate || 0).toFixed(1) });
    if (l) l.textContent = t('status.latency', { ms: state.latency == null ? '—' : state.latency });
    if (r && state.resolution) r.textContent = t('status.resolution', { w: state.resolution.width, h: state.resolution.height });
  }

  function refreshQualityDisplay() { paintStatus(); }

  /* ==================================================================
   * Remote toolbar
   * ================================================================== */

  function renderScreenMenu() {
    var menu = $('#screen-menu');
    if (!menu) return;
    menu.innerHTML = '';

    state.screens.forEach(function (screen) {
      var item = el('button', 'menu-item menu-item-screen');
      item.type = 'button';
      var ic = el('span', 'icon');
      ic.innerHTML = icons.icon('monitor', { size: 16 });
      item.appendChild(ic);
      var label = el('span', 'screen-label');
      label.appendChild(el('span', 'screen-name', screen.name));
      var sub = el('span', 'screen-sub', screen.width + '×' + screen.height + (screen.scaleFactor && screen.scaleFactor !== 1 ? '  @' + screen.scaleFactor + 'x' : ''));
      label.appendChild(sub);
      if (screen.primary) label.appendChild(el('span', 'badge', t('screen.primary')));
      item.appendChild(label);
      if (screen.index === state.screenIndex) item.classList.add('is-active');
      item.addEventListener('click', function () {
        switchScreen(screen.index);
        closeMenus();
      });
      menu.appendChild(item);
    });

    var sep = el('div', 'menu-sep');
    menu.appendChild(sep);

    var virt = el('button', 'menu-item');
    virt.type = 'button';
    var vic = el('span', 'icon');
    vic.innerHTML = icons.icon('layers', { size: 16 });
    virt.appendChild(vic);
    virt.appendChild(el('span', '', state.virtualScreen ? t('screen.virtual.remove') : t('screen.virtual.create')));
    virt.addEventListener('click', function () {
      toggleVirtualScreen();
      closeMenus();
    });
    menu.appendChild(virt);
  }

  function switchScreen(index) {
    state.screenIndex = index;
    bridge.call('switchScreen', { index: index }).then(function () {
      applyScreen(index);
      var screen = state.screens[index];
      toast(t('toast.screenSwitched', { name: screen ? screen.name : '#' + index }), 'info');
    });
  }

  function applyScreen(index) {
    var screen = state.screens[index];
    if (!screen) return;
    state.resolution = { width: screen.width, height: screen.height };
    var canvas = $('#remote-canvas');
    if (canvas) {
      canvas.width = screen.width;
      canvas.height = screen.height;
    }
    paintStatus();
    renderScreenMenu();
  }

  function toggleVirtualScreen() {
    state.virtualScreen = !state.virtualScreen;
    var btn = $('#virtual-screen-btn');
    if (btn) btn.setAttribute('aria-pressed', state.virtualScreen ? 'true' : 'false');
    if (state.virtualScreen) {
      state.screens = state.screens.concat([{
        index: state.screens.length, name: 'Virtual Display', width: 1920, height: 1080, scaleFactor: 1, primary: false, virtual: true
      }]);
      toast(t('toast.virtualScreenCreated'), 'success');
    } else {
      state.screens = state.screens.filter(function (s) { return !s.virtual; });
      toast(t('toast.virtualScreenRemoved'), 'info');
    }
    renderScreenMenu();
  }

  /**
   * The toolbar carries text labels next to the icons. When the window
   * is too narrow for them, drop to icons only and fall back to the
   * tooltips (the title attributes are already set on every button).
   */
  function syncToolbarDensity() {
    var bar = $('#remote-toolbar');
    if (!bar) return;
    var actions = $('.toolbar-actions', bar);
    if (!actions) return;
    var wasCompact = bar.classList.contains('is-compact');
    bar.classList.remove('is-compact');
    // Reading a size makes the browser lay the toolbar out again, so
    // the measurement below is of the toolbar with its labels on.  Without
    // it the old size is measured and the toolbar stays in the icon
    // mode even when there is room for the labels.
    if (wasCompact) void actions.offsetWidth;
    // Measure the part that scrolls, because that is the part that has
    // to fit.  The state and the disconnect button have their own room.
    if (actions.scrollWidth > actions.clientWidth + 1) {
      bar.classList.add('is-compact');
    }
  }

  function setToolbarVisible(visible) {
    state.toolbarVisible = visible;
    var bar = $('#remote-toolbar');
    if (!bar) return;
    bar.classList.toggle('is-hidden', !visible);
    var toggleBtn = $('#toolbar-toggle');
    if (toggleBtn) {
      toggleBtn.innerHTML = icons.icon(visible ? 'chevron-up' : 'chevron-down', { size: 18 });
      toggleBtn.setAttribute('title', visible ? t('remote.toolbar.hide') : t('remote.toolbar.show'));
    }
  }

  function setCursorStyle(style) {
    state.cursorStyle = style;
    var stage = $('#remote-stage');
    if (!stage) return;
    stage.style.cursor = style === 'none' ? 'none' : style;
    var icon = $('#cursor-icon');
    var map = { default: 'mouse-pointer', pointer: 'hand', crosshair: 'crosshair', text: 'cursor-text', none: 'eye-off' };
    if (icon) {
      icon.setAttribute('data-icon', map[style] || 'mouse-pointer');
      hydrateIcons(icon.parentNode);
    }
  }

  /* ==================================================================
   * Remote input capture
   *
   * The shapes mirror `InputEvent` in skycremote-core/src/protocol.rs.
   * ================================================================== */

  var KEY_CODES = {
    Backspace: 8, Tab: 9, Enter: 13, ShiftLeft: 16, ShiftRight: 16, ControlLeft: 17, ControlRight: 17,
    AltLeft: 18, AltRight: 18, CapsLock: 20, Escape: 27, Space: 32, PageUp: 33, PageDown: 34,
    End: 35, Home: 36, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Insert: 45, Delete: 46,
    MetaLeft: 91, MetaRight: 92, ContextMenu: 93, NumLock: 144, ScrollLock: 145, Pause: 19
  };

  function keyCodeFor(event) {
    if (KEY_CODES[event.key] !== undefined) return KEY_CODES[event.key];
    if (/^F\d{1,2}$/.test(event.key)) return 111 + parseInt(event.key.slice(1), 10);
    if (event.key.length === 1) {
      var upper = event.key.toUpperCase();
      var code = upper.charCodeAt(0);
      if (code >= 48 && code <= 57) return code;      // 0-9
      if (code >= 65 && code <= 90) return code;      // A-Z
      return 0;
    }
    return 0;
  }

  /** Map a browser pointer event into remote-screen pixel coordinates. */
  function pointerToRemote(event) {
    var canvas = $('#remote-canvas');
    if (!canvas) return { x: 0, y: 0 };
    var rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height) return { x: 0, y: 0 };
    var x = (event.clientX - rect.left) / rect.width * canvas.width;
    var y = (event.clientY - rect.top) / rect.height * canvas.height;
    return {
      x: Math.round(clamp(x, 0, canvas.width)),
      y: Math.round(clamp(y, 0, canvas.height))
    };
  }

  function sendInput(payload) {
    // MOCK: a real backend sends this over the wire as `Message::Input`.
    return bridge.call('sendInput', payload);
  }

  function wireRemoteInput() {
    var stage = $('#remote-stage');
    if (!stage) return;

    stage.addEventListener('keydown', function (e) {
      if (!state.connected) return;
      if (isTypingTarget(e.target)) return;
      e.preventDefault();

      if (state.keymapRecording) {
        completeKeymapRecording(e);
        return;
      }

      var code = keyCodeFor(e);
      if (e.key.length === 1) {
        // A character is sent as itself.  Sending it as a key as well
        // would type it twice, and typing it by key would depend on
        // the keyboard layout of the machine on the other end.
        sendInput({ type: 'Text', text: e.key });
      } else {
        sendInput({ type: 'KeyDown', key: code });
      }
    });

    stage.addEventListener('keyup', function (e) {
      if (!state.connected) return;
      if (isTypingTarget(e.target)) return;
      e.preventDefault();
      // Characters went out as characters, so there is no key to lift.
      if (e.key.length === 1) return;
      sendInput({ type: 'KeyUp', key: keyCodeFor(e) });
    });

    stage.addEventListener('mousemove', function (e) {
      if (!state.connected) return;
      var p = pointerToRemote(e);
      sendInput({ type: 'MouseMove', x: p.x, y: p.y });
    });

    stage.addEventListener('mousedown', function (e) {
      if (!state.connected) return;
      e.preventDefault();
      var p = pointerToRemote(e);
      var button = e.button === 1 ? 1 : (e.button === 2 ? 2 : 0);
      sendInput({ type: 'MouseDown', button: button, x: p.x, y: p.y });
    });

    stage.addEventListener('mouseup', function (e) {
      if (!state.connected) return;
      var p = pointerToRemote(e);
      var button = e.button === 1 ? 1 : (e.button === 2 ? 2 : 0);
      sendInput({ type: 'MouseUp', button: button, x: p.x, y: p.y });
    });

    stage.addEventListener('wheel', function (e) {
      if (!state.connected) return;
      e.preventDefault();
      var p = pointerToRemote(e);
      // 浏览器可能给像素（deltaMode 0）、行（1）、页（2）。后端按像素算，
      // 不换算的话 Firefox 那种「行」的量会小得看起来像没反应。
      var scale =
        e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 16 * 25 : 1;
      sendInput({
        type: 'Scroll',
        dx: e.deltaX * scale,
        dy: e.deltaY * scale,
        x: p.x,
        y: p.y
      });
    }, { passive: false });

    stage.addEventListener('contextmenu', function (e) {
      e.preventDefault();
      showContextMenu(e.clientX, e.clientY);
    });

    // Clipboard: Ctrl/Cmd+C and Ctrl/Cmd+V.
    document.addEventListener('copy', function (e) {
      if (!state.connected || !isTypingTarget(e.target)) return;
      var text = (e.clipboardData || window.clipboardData).getData('text');
      if (text) sendInput({ type: 'Clipboard', data: text });
    });

    document.addEventListener('paste', function (e) {
      if (!state.connected || !isTypingTarget(e.target)) return;
      var text = (e.clipboardData || window.clipboardData).getData('text');
      if (text) sendInput({ type: 'Clipboard', data: text });
    });
  }

  function isTypingTarget(node) {
    if (!node) return false;
    var tag = (node.tagName || '').toLowerCase();
    return tag === 'input' || tag === 'textarea' || node.isContentEditable;
  }

  /* ==================================================================
   * Context menu (Windows shell look)
   * ================================================================== */

  var CTX_ITEMS = [
    { type: 'item', icon: 'monitor', label: 'ctx.view' },
    { type: 'item', icon: 'sliders-horizontal', label: 'ctx.sortBy' },
    { type: 'item', icon: 'refresh', label: 'ctx.refresh' },
    { type: 'sep' },
    { type: 'item', icon: 'copy', label: 'ctx.paste' },
    { type: 'item', icon: 'folder', label: 'ctx.newFolder' },
    { type: 'sep' },
    { type: 'item', icon: 'terminal', label: 'ctx.openTerminal' },
    { type: 'item', icon: 'zap', label: 'ctx.run' },
    { type: 'sep' },
    { type: 'item', icon: 'info', label: 'ctx.properties' },
    { type: 'item', icon: 'display', label: 'ctx.displaySettings' },
    { type: 'item', icon: 'sliders', label: 'ctx.personalize' },
    { type: 'item', icon: 'activity', label: 'ctx.taskManager' }
  ];

  function showContextMenu(x, y) {
    var menu = $('#context-menu');
    if (!menu) return;
    menu.innerHTML = '';

    CTX_ITEMS.forEach(function (item) {
      if (item.type === 'sep') {
        menu.appendChild(el('div', 'ctx-sep'));
        return;
      }
      var btn = el('button', 'ctx-item');
      btn.type = 'button';
      var ic = el('span', 'icon ctx-icon');
      ic.innerHTML = icons.icon(item.icon, { size: 15 });
      btn.appendChild(ic);
      btn.appendChild(el('span', 'ctx-label', t(item.label)));
      var arrow = el('span', 'icon ctx-arrow');
      arrow.innerHTML = icons.icon('chevron-right', { size: 14 });
      btn.appendChild(arrow);
      btn.addEventListener('click', function () {
        hide(menu);
        handleContextAction(item.label);
      });
      menu.appendChild(btn);
    });

    show(menu);
    menu.style.left = '0px';
    menu.style.top = '0px';
    var rect = menu.getBoundingClientRect();
    var left = clamp(x, 4, global.innerWidth - rect.width - 4);
    var top = clamp(y, 4, global.innerHeight - rect.height - 4);
    menu.style.left = left + 'px';
    menu.style.top = top + 'px';
    menu.classList.add('is-in');
  }

  function handleContextAction(label) {
    if (label === 'ctx.refresh') {
      loadDevices();
      return;
    }
    if (label === 'ctx.openTerminal') {
      openTerminal();
      return;
    }
    if (label === 'ctx.displaySettings') {
      openSettings();
      return;
    }
    if (label === 'ctx.personalize') {
      openSettings();
      return;
    }
    // MOCK: the rest of the shell items need a real desktop backend.
    toast(t('toast.mockNotice'), 'info');
  }

  /* ==================================================================
   * File transfer
   * ================================================================== */

  function openFileTransfer() {
    hide($('#context-menu'));
    show($('#file-drawer'));
    state.fileOpen = true;
    syncDrawerClass();
    loadFiles(state.currentPath);
  }

  function closeFileTransfer() {
    hide($('#file-drawer'));
    state.fileOpen = false;
    syncDrawerClass();
  }

  /** Keep the toast column clear of an open drawer. */
  function syncDrawerClass() {
    var app = document.getElementById('app');
    if (!app) return;
    var drawerOpen = state.fileOpen || state.settingsOpen;
    app.classList.toggle('has-drawer', drawerOpen);
  }

  /**
   * Stack the floating panels (cloud keyboard, gamepad) so two open at
   * once do not sit on top of each other.
   */
  function syncOverlayStack() {
    var panels = $$('.overlay-panel').filter(function (p) { return !p.hidden; });
    var offset = 0;
    panels.forEach(function (panel) {
      panel.style.setProperty('--stack-offset', offset + 'px');
      // The panel above this one needs to clear its height.
      offset += panel.offsetHeight + 12;
    });
  }

  function loadFiles(path) {
    state.currentPath = path || '/';
    var pathLabel = $('#file-path');
    if (pathLabel) pathLabel.textContent = state.currentPath;
    var list = $('#file-list');

    // MOCK: real listing comes from the peer over the file-transfer channel.
    bridge.call('listFiles', { path: state.currentPath }).then(function (files) {
      renderFiles(files || []);
    }).catch(function () {
      if (list) list.innerHTML = '';
      toast(t('files.error.read'), 'error');
    });
  }

  function renderFiles(files) {
    var list = $('#file-list');
    if (!list) return;
    list.innerHTML = '';

    if (!files.length) {
      var empty = el('div', 'file-empty');
      empty.appendChild(el('span', 'icon', ''));
      empty.firstChild.innerHTML = icons.icon('folder', { size: 28 });
      empty.appendChild(el('p', '', t('files.empty')));
      list.appendChild(empty);
      return;
    }

    files.forEach(function (file) {
      var row = el('div', 'file-row');
      row.tabIndex = 0;

      var ic = el('span', 'icon file-icon');
      ic.innerHTML = icons.icon(file.isDir ? 'folder' : 'file', { size: 18 });
      row.appendChild(ic);

      var main = el('div', 'file-main');
      main.appendChild(el('div', 'file-name', file.name));
      var meta = el('div', 'file-meta');
      meta.appendChild(el('span', '', file.isDir ? t('files.folder') : formatBytes(file.size)));
      meta.appendChild(el('span', 'meta-sep', '·'));
      meta.appendChild(el('span', '', new Date(file.modified).toLocaleDateString()));
      main.appendChild(meta);
      row.appendChild(main);

      var act = el('button', 'icon-btn icon-btn-sm');
      act.type = 'button';
      act.innerHTML = icons.icon(file.isDir ? 'chevron-right' : 'download', { size: 16 });
      act.setAttribute('title', file.isDir ? t('common.more') : t('files.download'));
      act.addEventListener('click', function (e) {
        e.stopPropagation();
        if (file.isDir) {
          loadFiles(joinPath(state.currentPath, file.name));
        } else {
          queueTransfer(file.name, file.size, 'down');
          toast(t('files.downloading', { name: file.name }), 'info');
        }
      });
      row.appendChild(act);

      row.addEventListener('click', function () {
        if (file.isDir) loadFiles(joinPath(state.currentPath, file.name));
        else { queueTransfer(file.name, file.size, 'down'); toast(t('files.downloading', { name: file.name }), 'info'); }
      });
      row.addEventListener('keydown', function (e) {
        if (e.key === 'Enter') row.click();
      });

      list.appendChild(row);
    });
  }

  function joinPath(base, name) {
    if (base === '/') return '/' + name;
    return base.replace(/\/$/, '') + '/' + name;
  }

  /* ---- transfer queue ---- */

  var transferId = 0;

  function queueTransfer(name, size, direction) {
    var item = {
      id: ++transferId,
      name: name,
      size: size || 0,
      direction: direction || 'up',
      progress: 0,
      speed: 0,
      state: 'running'
    };
    state.fileQueue.push(item);
    renderQueue();

    // MOCK: a real backend streams FileChunk messages and reports progress.
    var timer = setInterval(function () {
      if (item.state !== 'running') { clearInterval(timer); return; }
      var step = 1.5 + Math.random() * 4;
      item.progress = Math.min(100, item.progress + step);
      // Mock throughput: fast enough that the ETA looks sane for any size.
      var throughput = Math.max(item.size / 6, 65536);
      item.speed = throughput * (step / 100);
      if (item.progress >= 100) {
        item.progress = 100;
        item.state = 'done';
        clearInterval(timer);
      }
      renderQueue();
    }, 320);

    item.cancel = function () {
      item.state = 'canceled';
      clearInterval(timer);
      renderQueue();
    };
    return item;
  }

  function renderQueue() {
    var host = $('#file-queue');
    var empty = $('#queue-empty');
    if (!host) return;
    host.innerHTML = '';

    if (!state.fileQueue.length) {
      if (empty) show(empty);
      return;
    }
    if (empty) hide(empty);

    state.fileQueue.forEach(function (item) {
      var row = el('div', 'queue-row');
      row.setAttribute('data-state', item.state);

      var head = el('div', 'queue-head');
      var ic = el('span', 'icon queue-icon');
      ic.innerHTML = icons.icon(item.direction === 'down' ? 'download' : 'upload', { size: 14 });
      head.appendChild(ic);
      head.appendChild(el('span', 'queue-name', item.name));
      head.appendChild(el('span', 'queue-pct', Math.round(item.progress) + '%'));
      row.appendChild(head);

      var bar = el('div', 'queue-bar');
      var fill = el('i');
      fill.style.width = item.progress + '%';
      bar.appendChild(fill);
      row.appendChild(bar);

      var foot = el('div', 'queue-foot');
      var label = item.state === 'running'
        ? t('files.item.' + (item.direction === 'down' ? 'downloading' : 'uploading'))
        : t('files.item.' + item.state);
      foot.appendChild(el('span', 'queue-state', label));
      foot.appendChild(el('span', 'queue-sep', '·'));
      foot.appendChild(el('span', '', formatBytes(item.size)));
      if (item.state === 'running') {
        foot.appendChild(el('span', 'queue-sep', '·'));
        foot.appendChild(el('span', '', formatRate(item.speed) + '/s'));
        foot.appendChild(el('span', 'queue-sep', '·'));
        var remaining = item.size * (1 - item.progress / 100);
        foot.appendChild(el('span', '', t('common.eta') + ' ' + formatEta(item.speed > 0 ? remaining / item.speed : 0)));
      }
      row.appendChild(foot);

      if (item.state === 'running') {
        var cancel = el('button', 'icon-btn icon-btn-xs queue-cancel');
        cancel.type = 'button';
        cancel.innerHTML = icons.icon('x', { size: 13 });
        cancel.setAttribute('title', t('common.cancel'));
        cancel.addEventListener('click', function () { item.cancel(); });
        row.appendChild(cancel);
      }
      host.appendChild(row);
    });
  }

  function wireFileDrop() {
    var zone = $('#file-dropzone');
    var input = $('#file-input');
    if (!zone) return;

    ['dragenter', 'dragover'].forEach(function (name) {
      zone.addEventListener(name, function (e) {
        e.preventDefault();
        e.stopPropagation();
        zone.classList.add('is-over');
      });
    });
    ['dragleave', 'drop'].forEach(function (name) {
      zone.addEventListener(name, function (e) {
        e.preventDefault();
        e.stopPropagation();
        zone.classList.remove('is-over');
      });
    });

    zone.addEventListener('drop', function (e) {
      var files = e.dataTransfer && e.dataTransfer.files;
      if (!files || !files.length) return;
      for (var i = 0; i < files.length; i++) {
        queueTransfer(files[i].name, files[i].size, 'up');
      }
      // MOCK: the real upload starts once the backend accepts the file.
      toast(t('toast.mockNotice'), 'info');
    });

    var uploadBtn = $('#file-upload-btn');
    if (uploadBtn && input) {
      uploadBtn.addEventListener('click', function () { input.click(); });
      input.addEventListener('change', function () {
        for (var i = 0; i < input.files.length; i++) {
          queueTransfer(input.files[i].name, input.files[i].size, 'up');
        }
        input.value = '';
      });
    }
  }

  /* ==================================================================
   * Terminal
   * ================================================================== */

  function openTerminal() {
    show($('#terminal-panel'));
    state.terminalOpen = true;
    if (!state.terminalLines.length) {
      terminalWrite(t('terminal.connected'));
    }
    var input = $('#terminal-input');
    if (input) input.focus();
  }

  function closeTerminal() {
    hide($('#terminal-panel'));
    state.terminalOpen = false;
  }

  function terminalWrite(text, cls) {
    var out = $('#terminal-output');
    if (!out) return;
    var line = el('div', 'term-line' + (cls ? ' ' + cls : ''), text);
    out.appendChild(line);
    state.terminalLines.push(text);
    out.scrollTop = out.scrollHeight;
  }

  function terminalExec(command) {
    if (!command) return;
    terminalWrite(t('terminal.prompt') + ' ' + command, 'term-cmd');
    // MOCK: a real terminal streams `Message::Terminal` lines back.
    bridge.call('terminalExec', { command: command }).then(function (res) {
      var reply = res && res.output ? res.output : '';
      if (!reply) {
        reply = 'command not found: ' + command;
      }
      reply.split('\n').forEach(function (line) { if (line) terminalWrite(line, 'term-out'); });
    });
  }

  /* ==================================================================
   * Cloud keyboard
   * ================================================================== */

  var CLOUD_KEYS = [
    ['Esc', 'Escape'], ['Tab', 'Tab'], ['Caps', 'CapsLock'], ['Shift', 'ShiftLeft'],
    ['Ctrl', 'ControlLeft'], ['Alt', 'AltLeft'], ['Del', 'Delete'], ['Ins', 'Insert'],
    ['Home', 'Home'], ['End', 'End'], ['PgUp', 'PageUp'], ['PgDn', 'PageDown'],
    ['↑', 'ArrowUp'], ['←', 'ArrowLeft'], ['↓', 'ArrowDown'], ['→', 'ArrowRight'],
    ['F1', 'F1'], ['F2', 'F2'], ['F3', 'F3'], ['F4', 'F4'],
    ['F5', 'F5'], ['F6', 'F6'], ['F7', 'F7'], ['F8', 'F8'],
    ['F9', 'F9'], ['F10', 'F10'], ['F11', 'F11'], ['F12', 'F12']
  ];

  function renderCloudKeys() {
    var host = $('#cloud-keys');
    if (!host) return;
    host.innerHTML = '';
    CLOUD_KEYS.forEach(function (pair) {
      var btn = el('button', 'cloud-key', pair[0]);
      btn.type = 'button';
      btn.addEventListener('click', function () {
        sendInput({ type: 'KeyDown', key: keyCodeFor({ key: pair[1] }) });
        sendInput({ type: 'KeyUp', key: keyCodeFor({ key: pair[1] }) });
      });
      host.appendChild(btn);
    });
  }

  /* ==================================================================
   * Gamepad
   * ================================================================== */

  function pollGamepads() {
    var host = $('#gamepad-list');
    if (!host) return;
    var pads = [];
    if (global.navigator && typeof navigator.getGamepads === 'function') {
      var raw = navigator.getGamepads();
      for (var i = 0; i < raw.length; i++) {
        if (raw[i]) pads.push(raw[i]);
      }
    }
    host.innerHTML = '';
    if (!pads.length) {
      var empty = el('div', 'gamepad-empty');
      var ic = el('span', 'icon');
      ic.innerHTML = icons.icon('gamepad', { size: 26 });
      empty.appendChild(ic);
      empty.appendChild(el('p', '', t('gamepad.none')));
      empty.appendChild(el('p', 'gamepad-hint', t('gamepad.connect')));
      host.appendChild(empty);
      return;
    }
    pads.forEach(function (pad) {
      var row = el('div', 'gamepad-row');
      var ic = el('span', 'icon');
      ic.innerHTML = icons.icon('gamepad', { size: 18 });
      row.appendChild(ic);
      row.appendChild(el('span', 'gamepad-name', t('gamepad.connected', {
        name: pad.id || 'Gamepad',
        buttons: pad.buttons ? pad.buttons.length : 0
      })));
      host.appendChild(row);
    });
  }

  /* ==================================================================
   * Keymap editor
   * ================================================================== */

  var KEYMAP_DEFAULTS = [
    { local: 'Ctrl+Alt+T', remote: 'Ctrl+Alt+T' },
    { local: 'Super', remote: 'MetaLeft' },
    { local: 'CapsLock', remote: 'CapsLock' }
  ];

  function renderKeymap() {
    var host = $('#keymap-rows');
    if (!host) return;
    host.innerHTML = '';

    var rows = state.settings.keymapRows && state.settings.keymapRows.length
      ? state.settings.keymapRows
      : KEYMAP_DEFAULTS;

    if (!rows.length) {
      var empty = el('div', 'keymap-empty');
      empty.appendChild(el('span', 'icon', ''));
      empty.firstChild.innerHTML = icons.icon('keyboard', { size: 26 });
      empty.appendChild(el('p', '', t('keymap.empty')));
      host.appendChild(empty);
      return;
    }

    rows.forEach(function (row, index) {
      var line = el('div', 'keymap-row');
      line.appendChild(el('span', 'keymap-local', row.local));
      line.appendChild(el('span', 'icon keymap-arrow', ''));
      line.lastChild.innerHTML = icons.icon('arrow-right', { size: 15 });
      line.appendChild(el('span', 'keymap-remote', row.remote));

      var record = el('button', 'btn btn-secondary btn-xs');
      record.type = 'button';
      record.textContent = t('keymap.record');
      record.addEventListener('click', function () {
        state.keymapRecording = { index: index };
        record.textContent = t('keymap.recording');
        record.classList.add('is-recording');
      });
      line.appendChild(record);

      var del = el('button', 'icon-btn icon-btn-xs');
      del.type = 'button';
      del.innerHTML = icons.icon('trash', { size: 14 });
      del.addEventListener('click', function () {
        rows.splice(index, 1);
        state.settings.keymapRows = rows.slice();
        renderKeymap();
      });
      line.appendChild(del);

      host.appendChild(line);
    });
  }

  function completeKeymapRecording(event) {
    var rec = state.keymapRecording;
    state.keymapRecording = null;
    if (!rec) return;
    var parts = [];
    if (event.ctrlKey) parts.push('Ctrl');
    if (event.altKey) parts.push('Alt');
    if (event.shiftKey) parts.push('Shift');
    if (event.metaKey) parts.push('Super');
    parts.push(event.key.length === 1 ? event.key.toUpperCase() : event.key);

    var rows = state.settings.keymapRows && state.settings.keymapRows.length
      ? state.settings.keymapRows
      : KEYMAP_DEFAULTS.slice();
    rows[rec.index] = { local: parts.join('+'), remote: event.key };
    state.settings.keymapRows = rows.slice();
    renderKeymap();
  }

  /* ==================================================================
   * Overlays
   * ================================================================== */

  function closeOverlays() {
    ['#cloud-keyboard', '#gamepad-panel', '#keymap-modal', '#confirm-modal'].forEach(function (sel) {
      hide($(sel));
    });
    syncOverlayStack();
    if (state.keymapRecording) {
      state.keymapRecording = null;
      renderKeymap();
    }
  }

  /* ==================================================================
   * Frame rendering
   * ================================================================== */

  function renderFrame(payload) {
    if (!payload) return;

    // The backend sends the picture as H.264.  The browser decodes it.
    if (payload.codec === 'h264' && payload.data) {
      var remote = $('#remote-canvas');
      if (!remote) return;
      if (!state.decoder) {
        if (!NS.Decoder.supported()) {
          if (window.console && console.warn) {
            console.warn('[skycremote] this browser cannot decode H.264');
          }
          return;
        }
        state.decoder = new NS.Decoder(remote);
      }
      state.decoder.feed(payload.data, payload.ptsNs || 0);
    }

    if (payload.imageData) {
      var canvas = $('#remote-canvas');
      if (!canvas) return;
      var ctx = canvas.getContext('2d');
      if (!ctx) return;
      var img = new ImageData(new Uint8ClampedArray(payload.imageData), payload.width, payload.height);
      ctx.putImageData(img, 0, 0);
    }
    if (payload.width && payload.height) {
      state.resolution = { width: payload.width, height: payload.height };
      paintStatus();
    }
  }

  /* ==================================================================
   * Public API for the Rust backend
   * ================================================================== */

  var eventHandlers = Object.create(null);

  var api = {
    /** Connect to a device by ID. */
    connect: function (deviceId) {
      startConnect(deviceId);
      return Promise.resolve();
    },

    /** Disconnect the current session. */
    disconnect: function () {
      if (!state.connected) return Promise.resolve();
      return bridge.call('disconnect').then(function () {
        state.connected = false;
        stopStatusPoll();
        bridge.mock.stopFrames();
        setConnectionStatus('closed');
        toast(t('toast.disconnected'), 'info');
        leaveRemoteView();
      });
    },

    /** Send a keyboard/mouse event to the peer. */
    sendInput: function (event) {
      return bridge.call('sendInput', event);
    },

    /** Switch the quality preset. */
    setQuality: function (id) {
      state.settings.quality = id;
      var sel = $('#quality-select');
      if (sel) sel.value = id;
      refreshQualityDisplay();
      persistSettings();
      return bridge.call('setQuality', { id: id });
    },

    /** Set the target bitrate in Mbps. */
    setBitrate: function (mbps) {
      var value = clamp(parseInt(mbps, 10) || 50, 2, 500);
      state.settings.bitrate = value;
      var slider = $('#bitrate-slider');
      if (slider) slider.value = value;
      var out = $('#bitrate-value');
      if (out) out.textContent = value + ' Mbps';
      persistSettings();
      return bridge.call('setBitrate', { mbps: value });
    },

    /** Set the target frame rate. */
    setFramerate: function (fps) {
      var value = clamp(parseInt(fps, 10) || 60, 1, 240);
      state.settings.framerate = value;
      var slider = $('#framerate-slider');
      if (slider) slider.value = value;
      var out = $('#framerate-value');
      if (out) out.textContent = value + ' FPS';
      persistSettings();
      return bridge.call('setFramerate', { fps: value });
    },

    /** List the screens the peer reports. */
    listScreens: function () {
      return bridge.call('listScreens').then(function (screens) {
        state.screens = normaliseScreens(screens || []);
        renderScreenMenu();
        return state.screens;
      });
    },

    /** Switch to a screen by index. */
    switchScreen: function (index) {
      switchScreen(index);
      return Promise.resolve();
    },

    /** Open the file transfer drawer. */
    openFileTransfer: function () {
      openFileTransfer();
      return Promise.resolve();
    },

    /** Open the terminal panel. */
    openTerminal: function () {
      openTerminal();
      return Promise.resolve();
    },

    /** Toggle the privacy screen on the peer. */
    togglePrivacyScreen: function () {
      state.privacyScreen = !state.privacyScreen;
      applyPrivacyScreen();
      return bridge.call('togglePrivacyScreen', { enabled: state.privacyScreen });
    },

    /** Remote power action: 'shutdown' | 'restart' | 'sleep' | 'logoff'. */
    powerAction: function (action) {
      return bridge.call('powerAction', { action: action }).then(function () {
        var labels = {
          shutdown: t('power.shutdown'), restart: t('power.restart'),
          sleep: t('power.sleep'), logoff: t('power.logoff')
        };
        toast(t('power.sent', { action: labels[action] || action }), 'info');
      });
    },

    /** Read the current settings. */
    getSettings: function () {
      return clone(state.settings);
    },

    /** Write settings back. */
    saveSettings: function (settings) {
      if (settings && typeof settings === 'object') {
        Object.keys(DEFAULT_SETTINGS).forEach(function (key) {
          if (Object.prototype.hasOwnProperty.call(settings, key)) state.settings[key] = settings[key];
        });
      }
      persistSettings();
      writeSettingsForm();
      applyTheme(state.settings.theme, false);
      i18n.setLocale(state.settings.language);
      applyI18n();
      return Promise.resolve(clone(state.settings));
    },

    /**
     * Subscribe to a backend event.
     * Events: 'connection' | 'frame' | 'toast' | 'screens' | 'terminal'
     *         | 'files' | 'settings' | 'disconnect'
     */
    on: function (eventName, callback) {
      if (typeof callback !== 'function') return function () {};
      if (!eventHandlers[eventName]) eventHandlers[eventName] = [];
      eventHandlers[eventName].push(callback);
      return function off() {
        var arr = eventHandlers[eventName];
        if (!arr) return;
        var i = arr.indexOf(callback);
        if (i !== -1) arr.splice(i, 1);
      };
    },

    /** Fire an event into the UI. The Rust side calls this. */
    emit: function (eventName, payload) {
      var handlers = eventHandlers[eventName];
      if (handlers) {
        handlers.slice().forEach(function (fn) {
          try { fn(payload); } catch (e) { if (console && console.error) console.error(e); }
        });
      }
      bridge.emit(eventName, payload);
    },

    // Extra conveniences used by the desktop shell.
    setLanguage: function (lang) {
      i18n.setLocale(lang);
      state.settings.language = lang;
      applyI18n();
      var sel = $('#settings-language');
      if (sel) sel.value = lang;
      persistSettings();
      syncToolbarDensity();
      return Promise.resolve(lang);
    },

    setTheme: function (theme) {
      applyTheme(theme, true);
      return Promise.resolve(theme);
    },

    showToast: function (message, kind) {
      return toast(message, kind);
    },

    getState: function () {
      return {
        view: state.view,
        connected: state.connected,
        connectionStatus: state.connectionStatus,
        screenIndex: state.screenIndex,
        latency: state.latency,
        bitrate: state.bitrate,
        framerate: state.framerate,
        privacyScreen: state.privacyScreen,
        virtualScreen: state.virtualScreen,
        backendIsNative: bridge.isNative()
      };
    }
  };

  /** Route a backend event to the matching UI behaviour. */
  function handleBackendEvent(eventName, payload) {
    switch (eventName) {
      case 'connection':
        handleConnectionEvent(payload);
        break;
      case 'frame':
        renderFrame(payload);
        break;
      case 'toast':
        if (payload && payload.message) toast(payload.message, payload.kind || 'info');
        break;
      case 'screens':
        state.screens = normaliseScreens((payload && payload.screens) || payload || []);
        renderScreenMenu();
        applyScreen(state.screenIndex);
        break;
      case 'terminal':
        if (payload && payload.data) {
          String(payload.data).split('\n').forEach(function (line) {
            if (line) terminalWrite(line, 'term-out');
          });
        }
        break;
      case 'files':
        renderFiles((payload && payload.files) || payload || []);
        break;
      case 'settings':
        if (payload) {
          Object.keys(DEFAULT_SETTINGS).forEach(function (key) {
            if (Object.prototype.hasOwnProperty.call(payload, key)) state.settings[key] = payload[key];
          });
          writeSettingsForm();
        }
        break;
      case 'disconnect':
        state.connected = false;
        stopStatusPoll();
        setConnectionStatus('closed');
        toast(t('toast.disconnected'), 'info');
        leaveRemoteView();
        break;
      default:
        break;
    }
  }

  function handleConnectionEvent(payload) {
    if (!payload) return;
    var status = payload.status || payload.state;
    if (status) setConnectionStatus(status);
    if (payload.latency !== undefined) state.latency = payload.latency;
    if (payload.bitrate !== undefined) state.bitrate = payload.bitrate;
    if (payload.framerate !== undefined) state.framerate = payload.framerate;
    if (payload.deviceName && status === 'connected') {
      toast(t('toast.connected', { name: payload.deviceName }), 'success');
    }
    if (status === 'reconnecting') {
      toast(t('toast.connectionLost'), 'warning');
    }
    paintStatus();
  }

  /* ==================================================================
   * Wiring
   * ================================================================== */

  function wireTopbar() {
    var langBtn = $('#toolbar-lang');
    var langMenu = $('#lang-menu');
    if (langBtn && langMenu) {
      langBtn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (openMenu && openMenu.menu === langMenu) closeMenus();
        else showMenu(langMenu, langBtn);
      });
      $$('[data-locale]', langMenu).forEach(function (btn) {
        btn.addEventListener('click', function () {
          var lang = btn.getAttribute('data-locale');
          i18n.setLocale(lang);
          state.settings.language = lang;
          var sel = $('#settings-language');
          if (sel) sel.value = lang;
          persistSettings();
          applyI18n();
          updateLangLabel();
          syncToolbarDensity();
          closeMenus();
        });
      });
    }

    var themeBtn = $('#toolbar-theme');
    var themeMenu = $('#theme-menu');
    if (themeBtn && themeMenu) {
      themeBtn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (openMenu && openMenu.menu === themeMenu) closeMenus();
        else showMenu(themeMenu, themeBtn);
      });
      $$('[data-theme-choice]', themeMenu).forEach(function (btn) {
        btn.addEventListener('click', function () {
          applyTheme(btn.getAttribute('data-theme-choice'), true);
          closeMenus();
        });
      });
    }

    var settingsBtn = $('#toolbar-settings');
    if (settingsBtn) settingsBtn.addEventListener('click', openSettings);
  }

  function updateLangLabel() {
    var label = $('#lang-label');
    if (label) label.textContent = i18n.getLocale() === 'zh' ? '中' : 'EN';
  }

  function wireConnectView() {
    var form = $('#connect-form');
    if (form) {
      form.addEventListener('submit', function (e) {
        e.preventDefault();
        startConnect($('#device-id-input').value);
      });
    }

    var input = $('#device-id-input');
    var clear = $('#id-field-clear');
    if (input && clear) {
      input.addEventListener('input', function () {
        clear.hidden = !input.value;
      });
      clear.addEventListener('click', function () {
        input.value = '';
        clear.hidden = true;
        input.focus();
      });
      // Autofocus once the DOM is ready.
      setTimeout(function () { input.focus(); }, 60);
    }

    $$('#device-tabs .seg').forEach(function (seg) {
      seg.addEventListener('click', function () {
        $$('#device-tabs .seg').forEach(function (s) { s.classList.remove('is-active'); });
        seg.classList.add('is-active');
        state.deviceTab = seg.getAttribute('data-tab');
        renderDevices();
      });
    });

    var search = $('#device-search');
    if (search) {
      search.addEventListener('input', debounce(function () {
        state.deviceQuery = search.value;
        renderDevices();
      }, 120));
    }

    var refresh = $('#device-refresh');
    if (refresh) refresh.addEventListener('click', loadDevices);
  }

  function wireRemoteToolbar() {
    var toggle = $('#toolbar-toggle');
    if (toggle) toggle.addEventListener('click', function () {
      state.toolbarPinned = !state.toolbarPinned;
      setToolbarVisible(!state.toolbarVisible);
      if (state.toolbarPinned) setToolbarVisible(true);
    });

    var screenBtn = $('#screen-btn');
    var screenMenu = $('#screen-menu');
    if (screenBtn && screenMenu) {
      screenBtn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (openMenu && openMenu.menu === screenMenu) closeMenus();
        else showMenu(screenMenu, screenBtn);
      });
    }

    var virtualBtn = $('#virtual-screen-btn');
    if (virtualBtn) virtualBtn.addEventListener('click', toggleVirtualScreen);

    var cursorBtn = $('#cursor-btn');
    var cursorMenu = $('#cursor-menu');
    if (cursorBtn && cursorMenu) {
      cursorBtn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (openMenu && openMenu.menu === cursorMenu) closeMenus();
        else showMenu(cursorMenu, cursorBtn);
      });
      $$('[data-cursor]', cursorMenu).forEach(function (btn) {
        btn.addEventListener('click', function () {
          setCursorStyle(btn.getAttribute('data-cursor'));
          closeMenus();
        });
      });
    }

    var powerBtn = $('#btn-power');
    var powerMenu = $('#power-menu');
    if (powerBtn && powerMenu) {
      powerBtn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (openMenu && openMenu.menu === powerMenu) closeMenus();
        else showMenu(powerMenu, powerBtn);
      });
      $$('[data-power]', powerMenu).forEach(function (btn) {
        btn.addEventListener('click', function () {
          var action = btn.getAttribute('data-power');
          closeMenus();
          confirmDialog(t('power.confirm.' + action)).then(function (ok) {
            if (!ok) return;
            api.powerAction(action);
          });
        });
      });
    }

    var fileBtn = $('#btn-file');
    if (fileBtn) fileBtn.addEventListener('click', openFileTransfer);

    var terminalBtn = $('#btn-terminal');
    if (terminalBtn) terminalBtn.addEventListener('click', openTerminal);

    var keymapBtn = $('#btn-keymap');
    if (keymapBtn) keymapBtn.addEventListener('click', function () {
      hide($('#context-menu'));
      renderKeymap();
      show($('#keymap-modal'));
    });

    var cloudBtn = $('#btn-cloud-keyboard');
    if (cloudBtn) cloudBtn.addEventListener('click', function () {
      show($('#cloud-keyboard'));
      syncOverlayStack();
    });

    var gamepadBtn = $('#btn-gamepad');
    if (gamepadBtn) gamepadBtn.addEventListener('click', function () {
      show($('#gamepad-panel'));
      pollGamepads();
      syncOverlayStack();
    });

    var privacyBtn = $('#btn-privacy');
    if (privacyBtn) privacyBtn.addEventListener('click', function () {
      api.togglePrivacyScreen();
    });

    var disconnectBtn = $('#btn-disconnect');
    if (disconnectBtn) disconnectBtn.addEventListener('click', function () {
      confirmDialog(t('power.confirm.logoff')).then(function (ok) {
        if (ok) api.disconnect();
      });
    });

    var fullscreenBtn = $('#fullscreen-btn');
    if (fullscreenBtn) {
      fullscreenBtn.addEventListener('click', function () {
        toggleFullscreen();
      });
    }
  }

  function toggleFullscreen() {
    var app = document.getElementById('app');
    var target = app || document.documentElement;
    if (!document.fullscreenElement) {
      if (target.requestFullscreen) target.requestFullscreen();
      document.body.classList.add('is-fullscreen');
    } else {
      if (document.exitFullscreen) document.exitFullscreen();
      document.body.classList.remove('is-fullscreen');
    }
  }

  /** Auto-show the toolbar when the pointer moves to the top of the stage. */
  function wireToolbarAutoShow() {
    var stage = $('#remote-stage');
    var bar = $('#remote-toolbar');
    if (!stage || !bar) return;

    var hideTimer = null;
    stage.addEventListener('mousemove', function (e) {
      var rect = stage.getBoundingClientRect();
      if (e.clientY - rect.top < 90) {
        clearTimeout(hideTimer);
        setToolbarVisible(true);
        if (state.toolbarPinned) return;
        hideTimer = setTimeout(function () {
          if (!state.toolbarPinned) setToolbarVisible(false);
        }, 2600);
      }
    });

    stage.addEventListener('mouseleave', function () {
      clearTimeout(hideTimer);
      if (!state.toolbarPinned) setToolbarVisible(false);
    });
  }

  function wireSettingsPanel() {
    var closeBtn = $('#settings-close');
    if (closeBtn) closeBtn.addEventListener('click', closeSettings);

    var saveBtn = $('#settings-save-btn');
    if (saveBtn) saveBtn.addEventListener('click', saveSettings);

    var resetBtn = $('#settings-reset-btn');
    if (resetBtn) resetBtn.addEventListener('click', resetSettings);

    var bitrate = $('#bitrate-slider');
    if (bitrate) {
      bitrate.addEventListener('input', function () {
        var out = $('#bitrate-value');
        if (out) out.textContent = bitrate.value + ' Mbps';
      });
      bitrate.addEventListener('change', function () { api.setBitrate(bitrate.value); });
    }

    var framerate = $('#framerate-slider');
    if (framerate) {
      framerate.addEventListener('input', function () {
        var out = $('#framerate-value');
        if (out) out.textContent = framerate.value + ' FPS';
      });
      framerate.addEventListener('change', function () { api.setFramerate(framerate.value); });
    }

    var quality = $('#quality-select');
    if (quality) quality.addEventListener('change', function () {
      state.settings.quality = quality.value;
      refreshQualityDisplay();
      toast(t('toast.qualityChanged', { value: t('status.quality.' + quality.value) }), 'info');
    });

    var encoder = $('#encoder-select');
    if (encoder) encoder.addEventListener('change', function () {
      state.settings.encoder = encoder.value;
      toast(t('toast.encoderChanged', { value: t('settings.encoder.' + encoder.value.replace('-', '')) || encoder.value }), 'info');
    });

    var language = $('#settings-language');
    if (language) language.addEventListener('change', function () {
      i18n.setLocale(language.value);
      state.settings.language = language.value;
      applyI18n();
      updateLangLabel();
    });

    var theme = $('#settings-theme');
    if (theme) theme.addEventListener('change', function () {
      applyTheme(theme.value, true);
    });

    var privacyToggle = $('#privacy-screen-toggle');
    if (privacyToggle) privacyToggle.addEventListener('change', function () {
      state.privacyScreen = privacyToggle.checked;
      applyPrivacyScreen();
    });

    var keymapEdit = $('#keymap-edit-btn');
    if (keymapEdit) keymapEdit.addEventListener('click', function () {
      renderKeymap();
      show($('#keymap-modal'));
    });

    var portAdd = $('#port-add-btn');
    if (portAdd) portAdd.addEventListener('click', function () {
      state.settings.portMappings = state.settings.portMappings || [];
      state.settings.portMappings.push({ localPort: 0, remoteHost: '', remotePort: 0 });
      renderPorts();
    });

    var browse = $('#file-browse-btn');
    if (browse) browse.addEventListener('click', function () {
      // MOCK: the real button opens a native folder picker.
      toast(t('toast.mockNotice'), 'info');
    });
  }

  function applyPrivacyScreen() {
    var shield = $('#privacy-shield');
    var btn = $('#btn-privacy');
    if (shield) shield.hidden = !state.privacyScreen;
    if (btn) btn.setAttribute('aria-pressed', state.privacyScreen ? 'true' : 'false');
    var toggleBox = $('#privacy-screen-toggle');
    if (toggleBox) toggleBox.checked = state.privacyScreen;
    toast(state.privacyScreen ? t('privacy.screen.on') : t('privacy.screen.off'), 'info');
  }

  function wireFileDrawer() {
    var close = $('#file-close');
    if (close) close.addEventListener('click', closeFileTransfer);

    var refresh = $('#file-refresh');
    if (refresh) refresh.addEventListener('click', function () { loadFiles(state.currentPath); });

    var up = $('#file-up');
    if (up) up.addEventListener('click', function () {
      var parts = state.currentPath.split('/').filter(Boolean);
      parts.pop();
      loadFiles('/' + parts.join('/'));
    });
  }

  function wireTerminal() {
    var close = $('#terminal-close');
    if (close) close.addEventListener('click', closeTerminal);

    var input = $('#terminal-input');
    var send = $('#terminal-send');
    function submit() {
      if (!input) return;
      var cmd = input.value.trim();
      input.value = '';
      terminalExec(cmd);
    }
    if (input) {
      input.addEventListener('keydown', function (e) {
        if (e.key === 'Enter') { e.preventDefault(); submit(); }
      });
    }
    if (send) send.addEventListener('click', submit);

    var copy = $('#terminal-copy');
    if (copy) copy.addEventListener('click', function () {
      var text = state.terminalLines.join('\n');
      if (global.navigator && navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(function () {
          toast(t('toast.terminalCopied'), 'success');
        }).catch(function () { toast(t('files.error.download'), 'error'); });
      } else {
        toast(t('toast.mockNotice'), 'info');
      }
    });

    var clear = $('#terminal-clear');
    if (clear) clear.addEventListener('click', function () {
      state.terminalLines = [];
      var out = $('#terminal-output');
      if (out) out.innerHTML = '';
    });
  }

  function wireKeymapModal() {
    var close = $('#keymap-close');
    if (close) close.addEventListener('click', function () {
      hide($('#keymap-modal'));
      state.keymapRecording = null;
    });

    var add = $('#keymap-add-btn');
    if (add) add.addEventListener('click', function () {
      var rows = state.settings.keymapRows && state.settings.keymapRows.length
        ? state.settings.keymapRows
        : KEYMAP_DEFAULTS.slice();
      rows.push({ local: t('keymap.unassigned'), remote: t('keymap.unassigned') });
      state.settings.keymapRows = rows.slice();
      renderKeymap();
    });

    var reset = $('#keymap-reset-btn');
    if (reset) reset.addEventListener('click', function () {
      state.settings.keymapRows = clone(KEYMAP_DEFAULTS);
      renderKeymap();
    });

    var save = $('#keymap-save-btn');
    if (save) save.addEventListener('click', function () {
      persistSettings();
      hide($('#keymap-modal'));
      toast(t('toast.keymapSaved'), 'success');
    });
  }

  function wireOverlays() {
    var cloudClose = $('#cloud-close');
    if (cloudClose) cloudClose.addEventListener('click', function () {
      hide($('#cloud-keyboard'));
      syncOverlayStack();
    });

    var cloudSend = $('#cloud-send');
    var cloudText = $('#cloud-text');
    function cloudSubmit() {
      if (!cloudText) return;
      var text = cloudText.value;
      if (!text) return;
      cloudText.value = '';
      sendInput({ type: 'Text', text: text });
    }
    if (cloudSend) cloudSend.addEventListener('click', cloudSubmit);
    if (cloudText) cloudText.addEventListener('keydown', function (e) {
      if (e.key === 'Enter') { e.preventDefault(); cloudSubmit(); }
    });

    var gamepadClose = $('#gamepad-close');
    if (gamepadClose) gamepadClose.addEventListener('click', function () {
      hide($('#gamepad-panel'));
      syncOverlayStack();
    });
  }

  /** Keep the canvas sized to its container and to the device pixel ratio. */
  function wireResizeHandling() {
    var stage = $('#remote-stage');
    var canvas = $('#remote-canvas');
    if (!stage || !canvas) return;

    function fit() {
      var rect = stage.getBoundingClientRect();
      if (!rect.width || !rect.height) return;
      var dpr = global.devicePixelRatio || 1;
      var cssWidth = rect.width;
      var cssHeight = rect.height;

      // Letterbox the remote picture inside the stage.
      var rw = canvas.width;
      var rh = canvas.height;
      var scale = Math.min(cssWidth / rw, cssHeight / rh);
      var drawW = rw * scale;
      var drawH = rh * scale;
      canvas.style.width = drawW + 'px';
      canvas.style.height = drawH + 'px';
      canvas.style.left = ((cssWidth - drawW) / 2) + 'px';
      canvas.style.top = ((cssHeight - drawH) / 2) + 'px';

      state.lastRemoteSize = { width: rect.width, height: rect.height, dpr: dpr };
    }

    if (global.ResizeObserver) {
      var ro = new ResizeObserver(fit);
      ro.observe(stage);
    }
    global.addEventListener('resize', fit);
    global.addEventListener('orientationchange', fit);
    global.addEventListener('resize', debounce(syncToolbarDensity, 120));

    // A zoom change on some platforms does not fire resize; poll the ratio.
    var lastDpr = global.devicePixelRatio || 1;
    global.setInterval(function () {
      var dpr = global.devicePixelRatio || 1;
      if (dpr !== lastDpr) {
        lastDpr = dpr;
        fit();
      }
    }, 1000);

    fit();
  }

  function wireGlobalKeys() {
    document.addEventListener('keydown', function (e) {
      // Ctrl/Cmd+Alt+F toggles fullscreen.
      if (e.ctrlKey && e.altKey && (e.key === 'f' || e.key === 'F')) {
        e.preventDefault();
        toggleFullscreen();
        return;
      }
      // Ctrl/Cmd+, opens settings.
      if ((e.ctrlKey || e.metaKey) && e.key === ',') {
        e.preventDefault();
        openSettings();
      }
    });

    if (systemDark && typeof systemDark.addEventListener === 'function') {
      systemDark.addEventListener('change', function () {
        if (state.themePref === 'system') applyTheme('system', false);
      });
    }
  }

  /* ==================================================================
   * Init
   * ================================================================== */

  function init() {
    loadSettings();
    hydrateIcons();
    applyI18n();
    updateLangLabel();
    writeSettingsForm();
    renderCloudKeys();
    renderQueue();

    wireTopbar();
    wireConnectView();
    wireRemoteToolbar();
    wireToolbarAutoShow();
    wireSettingsPanel();
    wireFileDrawer();
    wireTerminal();
    wireKeymapModal();
    wireOverlays();
    wireRemoteInput();
    wireResizeHandling();
    wireGlobalKeys();
    wireConfirm();
    wireFileDrop();

    // Backend events.
    bridge.on('connection', handleConnectionEvent);
    bridge.on('frame', renderFrame);
    bridge.on('toast', function (p) {
      if (p && p.message) toast(p.message, p.kind || 'info');
    });
    bridge.on('screens', function (p) {
      state.screens = normaliseScreens((p && p.screens) || p || []);
      renderScreenMenu();
      applyScreen(state.screenIndex);
    });
    bridge.on('terminal', function (p) {
      if (p && p.data) {
        String(p.data).split('\n').forEach(function (line) { if (line) terminalWrite(line, 'term-out'); });
      }
    });
    bridge.on('files', function (p) { renderFiles((p && p.files) || p || []); });
    bridge.on('disconnect', function () {
      state.connected = false;
      stopStatusPoll();
      setConnectionStatus('closed');
      toast(t('toast.disconnected'), 'info');
      leaveRemoteView();
    });

    // MOCK: seed the device list so the screen is not empty.
    loadDevices();

    // MOCK: the backend pushes the current settings on start.
    bridge.call('getSettings').then(function (s) {
      if (s && Object.keys(s).length) {
        Object.keys(DEFAULT_SETTINGS).forEach(function (key) {
          if (Object.prototype.hasOwnProperty.call(s, key)) state.settings[key] = s[key];
        });
        writeSettingsForm();
      }
    });

    global.skycremote = api;
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})(typeof window !== 'undefined' ? window : globalThis);
