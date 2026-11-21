/*!
 * SkycRemote UI — application
 *
 * Plain HTML/CSS/JS. No framework, no build step. Loaded as a classic
 * script so it runs from a file:// URL inside WKWebView / WebView2 /
 * WebKitGTK without a CORS dance.
 *
 * The window has two states and no menu in between: the list of
 * devices, and the remote picture.  The backend is reached through
 * components/bridge.js, which is a mock until the Rust side is
 * attached.
 */
(function (global) {
  'use strict';

  var NS = global.SkycUI;
  var icons = NS.icons;
  var i18n = NS.i18n;
  var bridge = NS.bridge;

  var t = i18n.t;

  /* ==================================================================
   * Settings
   * ================================================================== */

  // The presets are the numbers that are really used: the picture
  // quality is a frame rate and a bitrate, nothing else.
  var QUALITY_PRESETS = {
    auto: { fps: 30, bitrate: 8 },
    '4k': { fps: 60, bitrate: 200 },
    fps144: { fps: 144, bitrate: 120 },
    '1080p60': { fps: 60, bitrate: 20 },
    '1080p': { fps: 30, bitrate: 12 }
  };

  var DEFAULT_SETTINGS = {
    language: 'zh',
    theme: 'dark',
    tracker: 'http://127.0.0.1:21115',
    deviceId: '',
    deviceName: '',
    quality: 'auto',
    bitrate: 8,
    framerate: 30,
    // The code other people type to connect to this computer.
    accessCode: '',
    // The codes this computer has remembered, keyed by device id.
    savedCodes: {}
  };

  var state = {
    view: 'list',
    settings: clone(DEFAULT_SETTINGS),
    devices: [],
    deviceQuery: '',
    // Whether this machine is sharing its screen right now.
    sharing: false,
    hostState: 'stopped',
    hostVerifyCode: '',
    trackerInfo: null,
    connected: false,
    connecting: false,
    connectionStatus: 'idle',
    // The six digits the other side reads out.
    verifyCode: '',
    screens: [],
    screenIndex: 0,
    latency: null,
    bitrate: null,
    framerate: null,
    resolution: null,
    // The decoder for the pictures.
    decoder: null,
    logs: [],
    themePref: 'dark',
    resolvedTheme: 'dark',
    // Who is waiting for a code in the dialog right now.
    accessResolver: null
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

  function setText(node, text) {
    if (node) node.textContent = text === undefined || text === null ? '' : String(text);
    return node;
  }

  function clamp(value, lo, hi) { return Math.min(hi, Math.max(lo, value)); }

  function bind(node, event, handler) {
    if (node) node.addEventListener(event, handler);
    return node;
  }

  function normaliseScreens(list) {
    return (list || []).map(function (screen) {
      return bridge.normaliseScreen(screen);
    }).filter(Boolean);
  }

  /* ==================================================================
   * i18n and icons
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

  /** Replace every <span data-icon="name"> with its SVG. */
  function hydrateIcons(root) {
    $$('[data-icon]', root || document).forEach(function (node) {
      var name = node.getAttribute('data-icon');
      if (!name) return;
      node.innerHTML = icons.icon(name, { size: parseInt(node.getAttribute('data-icon-size'), 10) || 18 });
    });
  }

  /* ==================================================================
   * Theme
   * ================================================================== */

  var systemDark = global.matchMedia ? global.matchMedia('(prefers-color-scheme: dark)') : null;

  function resolveTheme(pref) {
    if (pref === 'light' || pref === 'dark') return pref;
    if (systemDark) return systemDark.matches ? 'dark' : 'light';
    return 'dark';
  }

  function applyTheme(pref, persist) {
    state.themePref = pref;
    state.resolvedTheme = resolveTheme(pref);
    document.documentElement.setAttribute('data-theme', state.resolvedTheme);

    var iconNode = $('#theme-icon');
    if (iconNode) {
      var wanted = state.resolvedTheme === 'dark' ? 'moon' : 'sun';
      if (iconNode.getAttribute('data-icon') !== wanted) {
        iconNode.setAttribute('data-icon', wanted);
        hydrateIcons(iconNode.parentNode);
      }
    }

    if (persist) {
      state.settings.theme = pref;
      var sel = $('#settings-theme');
      if (sel) sel.value = pref;
      persistSettings();
    }
  }

  /* ==================================================================
   * Notices
   * ================================================================== */

  var TOAST_ICON = {
    success: 'check-circle',
    warning: 'alert-triangle',
    error: 'x-circle',
    info: 'info'
  };

  function toast(message, kind, duration) {
    var host = $('#toast-container');
    if (!host) return null;
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
   * Menus
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

  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Escape') return;
    if (openMenu) { closeMenus(); return; }
    if (!$('#access-modal').hidden) { closeAccessDialog(null); return; }
    closeDrawer();
  });

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
      } catch (e) { /* corrupt, keep the defaults */ }
    }
    if (!state.settings.savedCodes || typeof state.settings.savedCodes !== 'object') {
      state.settings.savedCodes = {};
    }
    applyTheme(state.settings.theme || 'dark', false);
    i18n.setLocale(state.settings.language || i18n.detect());
  }

  /**
   * Push the settings to the backend.
   *
   * The file on disk belongs to the backend.  The browser's own
   * storage is only a copy, for the case where the page is opened
   * without the program behind it.
   */
  function persistSettings() {
    try {
      global.localStorage.setItem('skycremote.settings', JSON.stringify(state.settings));
    } catch (e) { /* ignore */ }

    return bridge.call('saveSettings', state.settings).then(function (result) {
      if (result && result.applied && result.applied.length) {
        result.applied.forEach(function (line) { toast(line, 'info'); });
      }
    }).catch(function () { /* the backend refused; the page keeps working */ });
  }

  /** Read every control in the settings drawer into the settings. */
  function readSettingsForm() {
    var s = state.settings;
    s.language = $('#settings-language').value;
    s.theme = $('#settings-theme').value;
    s.quality = $('#settings-quality').value;
    s.bitrate = parseInt($('#settings-bitrate').value, 10) || 8;
    s.framerate = parseInt($('#settings-framerate').value, 10) || 30;
    s.accessCode = ($('#settings-access-code').value || '').trim();
    s.tracker = ($('#settings-tracker').value || '').trim();
    s.deviceId = ($('#settings-device-id').value || '').trim();
    s.deviceName = $('#settings-device-name').value || '';
  }

  /** Push the settings back into the settings drawer. */
  function writeSettingsForm() {
    var s = state.settings;
    $('#settings-language').value = s.language || 'zh';
    $('#settings-theme').value = s.theme || 'dark';
    $('#settings-quality').value = s.quality || 'auto';
    $('#settings-bitrate').value = s.bitrate;
    $('#bitrate-value').textContent = s.bitrate + ' Mbps';
    $('#settings-framerate').value = s.framerate;
    $('#framerate-value').textContent = s.framerate + ' FPS';
    $('#settings-access-code').value = s.accessCode || '';
    $('#settings-tracker').value = s.tracker || '';
    $('#settings-device-id').value = s.deviceId || '';
    $('#settings-device-name').value = s.deviceName || '';
    renderSavedCodes();
  }

  function openSettings() {
    writeSettingsForm();
    show($('#settings-panel'));
    var scroll = $('#settings-scroll');
    if (scroll) scroll.scrollTop = 0;
  }

  function closeDrawer() {
    hide($('#settings-panel'));
    hide($('#log-panel'));
  }

  function saveSettings() {
    readSettingsForm();
    persistSettings();
    applyTheme(state.settings.theme, false);
    i18n.setLocale(state.settings.language);
    applyI18n();
    updateLangLabel();
    renderMyDevice();
    renderSavedCodes();
    paintStatus();
    toast(t('toast.settingsSaved'), 'success');
  }

  /** The codes this computer has remembered, one row each. */
  function renderSavedCodes() {
    var host = $('#saved-codes-list');
    if (!host) return;
    host.innerHTML = '';

    var codes = state.settings.savedCodes || {};
    var ids = Object.keys(codes);
    var empty = $('#saved-codes-empty');
    if (empty) empty.hidden = ids.length > 0;

    ids.forEach(function (id) {
      var row = el('div', 'saved-code-row');
      row.appendChild(el('span', 'saved-code-id mono', id));
      row.appendChild(el('span', 'saved-code-value mono', codes[id]));

      var del = el('button', 'icon-btn icon-btn-sm');
      del.type = 'button';
      del.innerHTML = icons.icon('trash', { size: 15 });
      del.setAttribute('title', t('common.remove'));
      del.addEventListener('click', function () {
        delete state.settings.savedCodes[id];
        persistSettings();
        renderSavedCodes();
      });
      row.appendChild(del);

      host.appendChild(row);
    });
  }

  /* ==================================================================
   * This computer
   * ================================================================== */

  function renderMyDevice() {
    var s = state.settings;

    setText($('#my-device-id'), s.deviceId || '—');
    setText($('#my-device-name'), s.deviceName || '—');

    var tracker = s.tracker || '—';
    if (state.trackerInfo && state.trackerInfo.lanUrl) tracker = state.trackerInfo.lanUrl;
    setText($('#my-tracker-value'), tracker);

    var statusText = t('home.share.stopped');
    var dotClass = '';
    if (state.hostState === 'sharing') { statusText = t('home.share.sharing'); dotClass = ' is-online'; }
    else if (state.hostState === 'waiting') { statusText = t('home.share.waiting'); }
    else if (state.hostState === 'warning') { statusText = t('home.share.warning'); dotClass = ' is-warning'; }
    setText($('#my-status-text'), statusText);

    var dot = $('#my-status-dot');
    if (dot) dot.className = 'status-dot' + dotClass;

    var accessRow = $('#my-access-code-row');
    if (accessRow) accessRow.hidden = !state.sharing;
    setText($('#my-access-code-value'), s.accessCode || t('home.myDevice.accessCodeNone'));

    var verifyRow = $('#my-verify-row');
    if (verifyRow) verifyRow.hidden = !state.hostVerifyCode;
    setText($('#my-verify-value'), state.hostVerifyCode || '—');

    var card = $('#my-device-card');
    if (card) card.classList.toggle('is-sharing', !!state.sharing);

    var label = $('#share-toggle-label');
    if (label) label.textContent = state.sharing ? t('home.myDevice.stopShare') : t('home.myDevice.share');

    var button = $('#share-toggle');
    if (button) {
      button.classList.toggle('is-on', !!state.sharing);
      var iconNode = button.querySelector('.icon');
      if (iconNode) {
        iconNode.setAttribute('data-icon', state.sharing ? 'stop-circle' : 'play');
        hydrateIcons(button);
      }
    }

    setText($('#share-note'), state.sharing ? t('home.myDevice.shareHint') : '');
  }

  function toggleShare() {
    if (state.sharing) {
      bridge.call('stopHost').then(function () {
        state.sharing = false;
        state.hostState = 'stopped';
        state.hostVerifyCode = '';
        renderMyDevice();
        toast(t('toast.hostStopped'), 'info');
      }).catch(function (error) {
        toast(errorMessage(error), 'error');
      });
      return;
    }

    bridge.call('startHost').then(function (result) {
      state.sharing = true;
      state.hostState = 'waiting';
      if (result && result.deviceId) state.settings.deviceId = result.deviceId;
      renderMyDevice();
    }).catch(function (error) {
      toast(errorMessage(error), 'error');
    });
  }

  function copyText(text) {
    if (!text) return;
    if (global.navigator && global.navigator.clipboard && global.navigator.clipboard.writeText) {
      global.navigator.clipboard.writeText(text).then(function () {
        toast(t('toast.copied'), 'success');
      }).catch(function () {
        legacyCopy(text);
      });
      return;
    }
    legacyCopy(text);
  }

  function legacyCopy(text) {
    var field = document.createElement('textarea');
    field.value = text;
    field.setAttribute('readonly', 'readonly');
    field.style.position = 'fixed';
    field.style.opacity = '0';
    document.body.appendChild(field);
    field.select();
    var done = false;
    try { done = document.execCommand('copy'); } catch (e) { done = false; }
    document.body.removeChild(field);
    toast(done ? t('toast.copied') : t('common.copyFailed'), done ? 'success' : 'warning');
  }

  /* ==================================================================
   * The list of other computers
   * ================================================================== */

  function renderDevices() {
    var list = $('#device-list');
    var empty = $('#device-empty');
    if (!list) return;

    var query = state.deviceQuery.trim().toLowerCase();
    var devices = state.devices.filter(function (device) {
      if (!query) return true;
      var hay = ((device.name || '') + ' ' + (device.id || '')).toLowerCase();
      return hay.indexOf(query) !== -1;
    });

    list.innerHTML = '';

    if (!devices.length) {
      show(empty);
      setText($('#device-empty-title'), query ? t('home.remote.searchEmpty') : t('home.remote.empty'));
      setText($('#device-empty-hint'), query ? '' : t('home.remote.emptyHint'));
      hide($('#device-note'));
      return;
    }
    hide(empty);

    devices.forEach(function (device) {
      list.appendChild(deviceRow(device));
    });

    // One device and it is this computer: say so, so that an empty
    // list is not read as a broken one.
    var onlySelf = devices.length === 1 && isSelf(devices[0]);
    var note = $('#device-note');
    if (note) {
      note.hidden = !onlySelf;
      if (onlySelf) note.textContent = t('home.remote.onlySelf');
    }
  }

  function isSelf(device) {
    return !!(device && device.id && state.settings.deviceId && device.id === state.settings.deviceId);
  }

  function deviceRow(device) {
    var row = el('div', 'device-row');
    row.setAttribute('role', 'listitem');
    row.tabIndex = 0;

    var avatar = el('div', 'device-avatar');
    var iconNode = el('span', 'icon');
    iconNode.innerHTML = icons.icon('monitor', { size: 20 });
    avatar.appendChild(iconNode);

    var main = el('div', 'device-main');
    var nameLine = el('div', 'device-name-line');
    nameLine.appendChild(el('span', 'device-name', device.name || device.id || ''));

    if (isSelf(device)) nameLine.appendChild(el('span', 'badge', t('home.remote.self')));

    var status = el('span', 'device-status' + (device.online ? ' is-online' : ''));
    status.appendChild(el('i', 'status-dot'));
    status.appendChild(el('span', '', device.online ? t('home.remote.online') : t('home.remote.offline')));
    nameLine.appendChild(status);
    main.appendChild(nameLine);

    main.appendChild(el('div', 'device-meta', device.id || ''));
    row.appendChild(avatar);
    row.appendChild(main);

    function go() { requestConnect(device.id); }
    row.addEventListener('click', go);
    row.addEventListener('keydown', function (e) {
      if (e.key !== 'Enter' && e.key !== ' ') return;
      e.preventDefault();
      go();
    });

    return row;
  }

  function loadDevices() {
    return bridge.call('listDevices').then(function (devices) {
      state.devices = Array.isArray(devices) ? devices : [];
      renderDevices();
    }).catch(function () {
      state.devices = [];
      renderDevices();
    });
  }

  /* ==================================================================
   * The access code
   * ================================================================== */

  /**
   * Ask for the access code of a device.
   * @returns {Promise<{code: string, remember: boolean}|null>}
   */
  function openAccessDialog(deviceId) {
    var modal = $('#access-modal');
    var input = $('#access-input');
    var remember = $('#access-remember');
    var label = $('#access-device');
    var hint = $('#access-hint');
    var title = $('#access-title');

    var device = findDevice(deviceId);
    var name = device && device.name ? device.name : deviceId;

    if (label) label.textContent = t('access.device') + ': ' + name;
    if (title) title.textContent = t('access.title');
    if (input) input.value = '';
    if (remember) remember.checked = true;
    if (hint) hint.textContent = bridge.isNative() ? '' : t('access.mockHint');

    show(modal);
    if (input) setTimeout(function () { input.focus(); }, 30);

    return new Promise(function (resolve) {
      state.accessResolver = resolve;
    });
  }

  function closeAccessDialog(answer) {
    hide($('#access-modal'));
    var resolve = state.accessResolver;
    state.accessResolver = null;
    if (resolve) resolve(answer);
  }

  /**
   * Connect to a device.  A code that was remembered is used straight
   * away; otherwise the person is asked for it.
   */
  function requestConnect(deviceId) {
    var id = (deviceId || '').trim();
    if (!id) return;

    var device = findDevice(id);
    if (device && device.online === false) {
      toast(t('home.remote.offlineToast'), 'warning');
      return;
    }

    var saved = state.settings.savedCodes ? state.settings.savedCodes[id] : null;
    if (saved) {
      connectTo(id, saved);
      return;
    }

    openAccessDialog(id).then(function (answer) {
      if (!answer) return;
      if (answer.remember && answer.code) {
        state.settings.savedCodes[id] = answer.code;
        persistSettings();
        renderSavedCodes();
        toast(t('toast.codeSaved'), 'success');
      }
      connectTo(id, answer.code);
    });
  }

  function connectTo(deviceId, accessCode) {
    if (state.connecting) return;
    state.connecting = true;
    setConnectionStatus('connecting');

    var button = $('#device-connect-btn');
    if (button) button.disabled = true;

    bridge.call('connect', { deviceId: deviceId, accessCode: accessCode || '' }).then(function () {
      state.connecting = false;
      if (button) button.disabled = false;
      enterRemoteView(deviceId);
    }).catch(function (error) {
      state.connecting = false;
      if (button) button.disabled = false;
      setConnectionStatus('idle');
      var code = error && error.code;
      if (code === 'accessDenied') toast(t('access.wrong'), 'error');
      else if (code === 'noAccessCode') toast(t('access.missing'), 'error');
      else toast(errorMessage(error), 'error');
    });
  }

  function errorMessage(error) {
    if (!error) return t('access.failed');
    if (error.code === 'accessDenied') return t('access.wrong');
    if (error.code === 'noAccessCode') return t('access.missing');
    if (error.message) return error.message;
    return t('access.failed');
  }

  /* ==================================================================
   * The remote picture
   * ================================================================== */

  function enterRemoteView(deviceId) {
    state.view = 'remote';
    $('#app').setAttribute('data-view', 'remote');
    $('#view-list').classList.remove('is-active');
    $('#view-remote').classList.add('is-active');

    state.connected = true;
    setConnectionStatus('connected');
    hide($('#stage-placeholder'));
    var toolbar = $('#remote-toolbar');
    if (toolbar) toolbar.hidden = false;

    var device = findDevice(deviceId);
    toast(t('toast.connected', { name: device && device.name ? device.name : deviceId }), 'success');

    // Without the real backend there is no video, so the mock paints
    // something on the canvas instead.
    var canvas = $('#remote-canvas');
    if (canvas && !bridge.isNative()) bridge.mock.startFrames(canvas);

    bridge.call('listScreens').then(function (screens) {
      state.screens = normaliseScreens(screens);
      renderScreenMenu();
    }).catch(function () { /* no screens yet */ });

    var stage = $('#remote-stage');
    if (stage) stage.focus();
  }

  function leaveRemoteView() {
    state.view = 'list';
    $('#app').setAttribute('data-view', 'list');
    $('#view-remote').classList.remove('is-active');
    $('#view-list').classList.add('is-active');

    state.connected = false;
    state.decoder = null;
    state.verifyCode = '';
    state.resolution = null;
    bridge.mock.stopFrames();
    show($('#stage-placeholder'));
    renderVerify();

    var list = $('#list-main');
    if (list) list.scrollTop = 0;
    loadDevices();
    renderMyDevice();
  }

  function disconnect() {
    var done = function () {
      leaveRemoteView();
      toast(t('toast.disconnected'), 'info');
    };
    if (!state.connected) { done(); return; }
    bridge.call('disconnect').then(done).catch(done);
  }

  function findDevice(id) {
    for (var i = 0; i < state.devices.length; i++) {
      if (state.devices[i].id === id) return state.devices[i];
    }
    return null;
  }

  function setConnectionStatus(status) {
    state.connectionStatus = status;
    var placeholder = $('#stage-placeholder');
    if (placeholder) {
      var text = placeholder.querySelector('p');
      if (text) {
        if (status === 'connecting') text.textContent = t('remote.connecting');
        else if (status === 'connected') text.textContent = t('remote.placeholder');
        else text.textContent = t('remote.placeholder');
      }
      if (status === 'connected') hide(placeholder);
    }
  }

  function renderVerify() {
    var box = $('#remote-verify');
    if (box) box.hidden = !state.verifyCode;
    setText($('#remote-verify-code'), state.verifyCode || '------');
    setText($('#remote-verify-hint'), t('remote.verify.hint'));
  }

  function paintStatus() {
    var quality = state.settings.quality;
    var qualityText = '—';
    if (quality && t('settings.quality.' + quality) !== 'settings.quality.' + quality) {
      qualityText = t('settings.quality.' + quality);
    }
    setText($('#stat-quality'), qualityText);
    setText($('#stat-framerate'), state.framerate === null ? '—' : state.framerate + ' FPS');
    setText($('#stat-bitrate'), state.bitrate === null ? '—' : state.bitrate + ' Mbps');
    setText($('#stat-latency'), state.latency === null ? '—' : Math.round(state.latency) + ' ms');
    setText($('#stat-resolution'), state.resolution
      ? state.resolution.width + '×' + state.resolution.height
      : '—');
  }

  function renderFrame(payload) {
    if (!payload) return;

    // The backend sends the picture as H.264 in one piece of text.
    if (typeof payload.data === 'string' && payload.data.length) {
      var remote = $('#remote-canvas');
      if (!remote) return;
      if (!state.decoder) {
        if (!NS.Decoder.supported()) {
          if (global.console && global.console.warn) {
            global.console.warn('[skycremote] this browser cannot decode H.264');
          }
          return;
        }
        state.decoder = new NS.Decoder(remote);
      }
      state.decoder.feed(payload.data, payload.ptsNs || 0);
    }

    if (payload.width && payload.height) {
      state.resolution = { width: payload.width, height: payload.height };
      paintStatus();
    }
  }

  /* ==================================================================
   * The toolbar
   * ================================================================== */

  function renderScreenMenu() {
    var menu = $('#screen-menu');
    if (!menu) return;
    menu.innerHTML = '';

    state.screens.forEach(function (screen) {
      var item = el('button', 'menu-item');
      item.type = 'button';
      item.setAttribute('role', 'menuitem');
      var iconNode = el('span', 'icon');
      iconNode.innerHTML = icons.icon('display', { size: 16 });
      item.appendChild(iconNode);
      item.appendChild(el('span', '', screen.name || ('Screen ' + (screen.index + 1))));
      if (screen.index === state.screenIndex) {
        item.appendChild(el('span', 'icon', ''));
      }
      item.addEventListener('click', function () {
        closeMenus();
        switchScreen(screen.index);
      });
      menu.appendChild(item);
    });

    if (!state.screens.length) {
      var empty = el('button', 'menu-item');
      empty.type = 'button';
      empty.disabled = true;
      empty.textContent = t('remote.placeholder');
      menu.appendChild(empty);
    }
  }

  function switchScreen(index) {
    bridge.call('switchScreen', { index: index }).then(function () {
      state.screenIndex = index;
      var screen = null;
      for (var i = 0; i < state.screens.length; i++) {
        if (state.screens[i].index === index) screen = state.screens[i];
      }
      toast(t('toast.screenSwitched', { name: screen ? screen.name : index }), 'success');
      renderScreenMenu();
    }).catch(function (error) {
      toast(errorMessage(error), 'error');
    });
  }

  function toggleFullscreen() {
    var stage = $('#remote-stage');
    if (!stage) return;
    var button = $('#fullscreen-btn');
    if (document.fullscreenElement) {
      if (document.exitFullscreen) document.exitFullscreen();
      return;
    }
    if (stage.requestFullscreen) {
      stage.requestFullscreen().then(function () {
        if (button) {
          button.setAttribute('data-i18n-title', 'remote.toolbar.exitFullscreen');
          button.setAttribute('title', t('remote.toolbar.exitFullscreen'));
        }
      }).catch(function () { /* the window said no */ });
    }
  }

  /** The parts of the program that the backend does not have yet. */
  function notReady() {
    toast(t('remote.notReady') + ' — ' + t('remote.notReadyHint'), 'warning');
  }

  /* ==================================================================
   * Sending input to the other side
   * ================================================================== */

  function sendInput(payload) {
    if (!state.connected) return;
    bridge.call('sendInput', payload).catch(function () { /* the other end is gone */ });
  }

  /** Where the pointer is, in the pixels of the picture that was sent. */
  function remotePoint(event) {
    var canvas = $('#remote-canvas');
    if (!canvas) return null;
    var rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height || !canvas.width || !canvas.height) return null;

    var scale = Math.min(rect.width / canvas.width, rect.height / canvas.height);
    var drawWidth = canvas.width * scale;
    var drawHeight = canvas.height * scale;
    var left = (rect.width - drawWidth) / 2;
    var top = (rect.height - drawHeight) / 2;

    var x = Math.round((event.clientX - rect.left - left) / scale);
    var y = Math.round((event.clientY - rect.top - top) / scale);
    return {
      x: clamp(x, 0, canvas.width),
      y: clamp(y, 0, canvas.height)
    };
  }

  function mouseButton(event) {
    if (event.button === 1) return 1;
    if (event.button === 2) return 2;
    return 0;
  }

  var lastMove = 0;

  function wireRemoteInput() {
    var stage = $('#remote-stage');
    if (!stage) return;

    stage.addEventListener('mousemove', function (event) {
      if (!state.connected) return;
      var now = Date.now();
      if (now - lastMove < 30) return;
      lastMove = now;
      var point = remotePoint(event);
      if (point) sendInput({ type: 'MouseMove', x: point.x, y: point.y });
    });

    stage.addEventListener('mousedown', function (event) {
      if (!state.connected) return;
      event.preventDefault();
      var point = remotePoint(event);
      if (!point) return;
      sendInput({ type: 'MouseDown', button: mouseButton(event), x: point.x, y: point.y });
    });

    stage.addEventListener('mouseup', function (event) {
      if (!state.connected) return;
      var point = remotePoint(event);
      if (!point) return;
      sendInput({ type: 'MouseUp', button: mouseButton(event), x: point.x, y: point.y });
    });

    stage.addEventListener('wheel', function (event) {
      if (!state.connected) return;
      event.preventDefault();
      var point = remotePoint(event);
      if (!point) return;
      var factor = event.deltaMode === 1 ? 16 : (event.deltaMode === 2 ? 100 : 1);
      sendInput({
        type: 'Scroll',
        dx: -event.deltaX * factor,
        dy: -event.deltaY * factor,
        x: point.x,
        y: point.y
      });
    }, { passive: false });

    stage.addEventListener('contextmenu', function (event) {
      if (state.connected) event.preventDefault();
    });

    document.addEventListener('keydown', function (event) {
      if (!state.connected) return;
      if (isTypingTarget(event.target)) return;
      if (event.metaKey || event.ctrlKey || event.altKey) return;

      event.preventDefault();
      sendInput({ type: 'KeyDown', key: event.keyCode || 0 });

      var text = event.key;
      if (text && text.length === 1) {
        sendInput({ type: 'Text', text: text });
      }
    });

    document.addEventListener('keyup', function (event) {
      if (!state.connected) return;
      if (isTypingTarget(event.target)) return;
      sendInput({ type: 'KeyUp', key: event.keyCode || 0 });
    });
  }

  function isTypingTarget(node) {
    if (!node || !node.tagName) return false;
    var tag = node.tagName.toLowerCase();
    return tag === 'input' || tag === 'textarea' || tag === 'select' || node.isContentEditable;
  }

  /* ==================================================================
   * The log
   * ================================================================== */

  function openLogs() {
    show($('#log-panel'));
    loadLogs();
  }

  function loadLogs() {
    return bridge.call('getLogs').then(function (result) {
      state.logs = (result && result.entries) || [];
      renderLogs();
    }).catch(function () {
      renderLogs();
    });
  }

  function renderLogs() {
    var host = $('#log-list');
    if (!host) return;
    host.innerHTML = '';

    if (!state.logs.length) {
      host.appendChild(el('p', 'log-empty', t('log.empty')));
    } else {
      state.logs.forEach(function (entry) {
        var row = el('div', 'log-row log-' + (entry.level || 'info'));
        row.appendChild(el('span', 'log-time', logTime(entry.time)));
        row.appendChild(el('span', 'log-level', entry.level || 'info'));
        row.appendChild(el('span', 'log-message', entry.message || ''));
        host.appendChild(row);
      });
    }

    setText($('#log-count'), t('log.count', { n: state.logs.length }));
  }

  function logTime(value) {
    var date = new Date(typeof value === 'number' ? value : Date.now());
    function two(part) { return (part < 10 ? '0' : '') + part; }
    return two(date.getHours()) + ':' + two(date.getMinutes()) + ':' + two(date.getSeconds());
  }

  function clearLogs() {
    bridge.call('clearLogs').then(function () {
      state.logs = [];
      renderLogs();
      toast(t('toast.logCleared'), 'success');
    }).catch(function (error) {
      toast(errorMessage(error), 'error');
    });
  }

  /* ==================================================================
   * Backend events
   * ================================================================== */

  function handleConnectionEvent(payload) {
    if (!payload) return;

    var status = payload.status || payload.state;
    if (status) setConnectionStatus(status);

    if (payload.latencyMs !== undefined) state.latency = payload.latencyMs;
    else if (payload.latency !== undefined) state.latency = payload.latency;
    if (payload.bitrate !== undefined) state.bitrate = payload.bitrate;
    if (payload.framerate !== undefined) state.framerate = payload.framerate;

    if (payload.verifyCode) {
      state.verifyCode = payload.verifyCode;
      renderVerify();
    }

    if (status === 'closed') {
      state.connected = false;
      leaveRemoteView();
    }

    paintStatus();
  }

  function handleHostEvent(payload) {
    if (!payload) return;

    var hostState = payload.state || 'stopped';
    state.hostState = hostState;
    state.sharing = (hostState === 'waiting' || hostState === 'sharing');

    if (payload.verifyCode) state.hostVerifyCode = payload.verifyCode;
    if (hostState === 'stopped') state.hostVerifyCode = '';

    renderMyDevice();

    if (hostState === 'sharing') toast(t('toast.hostStarted'), 'success');
    else if (hostState === 'stopped') toast(t('toast.hostStopped'), 'info');
    else if (hostState === 'warning') toast(payload.message || t('toast.hostWarning'), 'warning');
  }

  function handleSettingsEvent(payload) {
    if (!payload) return;
    state.backendSettings = payload;

    // The file on disk is the truth.  A key that is not in it is one
    // this version of the backend does not know about, so the copy the
    // page keeps is left alone.
    Object.keys(payload).forEach(function (key) {
      if (key === 'savedCodes') {
        if (payload.savedCodes && typeof payload.savedCodes === 'object') {
          state.settings.savedCodes = payload.savedCodes;
        }
        return;
      }
      if (key === 'accessCode') {
        state.settings.accessCode = payload.accessCode || '';
        return;
      }
      state.settings[key] = payload[key];
    });

    writeSettingsForm();
    renderMyDevice();
    paintStatus();
  }

  function handleBackendEvent(eventName, payload) {
    switch (eventName) {
      case 'connection':
        handleConnectionEvent(payload);
        break;
      case 'frame':
        renderFrame(payload);
        break;
      case 'host':
        handleHostEvent(payload);
        break;
      case 'settings':
        handleSettingsEvent(payload);
        break;
      case 'screens':
        state.screens = normaliseScreens((payload && payload.screens) || payload || []);
        renderScreenMenu();
        break;
      case 'toast':
        if (payload && payload.message) toast(payload.message, payload.kind || 'info');
        break;
      case 'disconnect':
        state.connected = false;
        leaveRemoteView();
        toast(t('toast.disconnected'), 'info');
        break;
      case 'quit':
        toast(t('toast.quit'), 'info');
        break;
      default:
        break;
    }
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
          renderMyDevice();
          renderDevices();
          renderSavedCodes();
          renderVerify();
          paintStatus();
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

    bind($('#toolbar-logs'), 'click', openLogs);
    bind($('#toolbar-settings'), 'click', openSettings);
  }

  function updateLangLabel() {
    var label = $('#lang-label');
    if (label) label.textContent = i18n.getLocale() === 'zh' ? '中' : 'EN';
  }

  function wireMyDevice() {
    bind($('#share-toggle'), 'click', toggleShare);
    bind($('#my-copy-id'), 'click', function () {
      copyText(state.settings.deviceId);
    });
  }

  function wireDeviceList() {
    bind($('#device-refresh'), 'click', function () { loadDevices(); });

    bind($('#device-search'), 'input', function (event) {
      state.deviceQuery = event.target.value || '';
      renderDevices();
    });

    var form = $('#manual-form');
    if (form) {
      form.addEventListener('submit', function (event) {
        event.preventDefault();
        var id = ($('#device-id-input').value || '').trim();
        if (!id) return;
        $('#device-id-input').value = '';
        requestConnect(id);
      });
    }
  }

  function wireAccessDialog() {
    bind($('#access-ok'), 'click', function () {
      var code = ($('#access-input').value || '').trim();
      var remember = $('#access-remember').checked;
      closeAccessDialog({ code: code, remember: remember });
    });

    bind($('#access-cancel'), 'click', function () {
      closeAccessDialog(null);
    });

    var input = $('#access-input');
    if (input) {
      input.addEventListener('keydown', function (event) {
        if (event.key !== 'Enter') return;
        event.preventDefault();
        var code = (input.value || '').trim();
        var remember = $('#access-remember').checked;
        closeAccessDialog({ code: code, remember: remember });
      });
    }
  }

  function wireRemoteToolbar() {
    bind($('#remote-back'), 'click', disconnect);
    bind($('#btn-disconnect'), 'click', disconnect);
    bind($('#fullscreen-btn'), 'click', toggleFullscreen);

    var screenBtn = $('#screen-btn');
    var screenMenu = $('#screen-menu');
    if (screenBtn && screenMenu) {
      screenBtn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (openMenu && openMenu.menu === screenMenu) closeMenus();
        else showMenu(screenMenu, screenBtn);
      });
    }

    var moreBtn = $('#more-btn');
    var moreMenu = $('#more-menu');
    if (moreBtn && moreMenu) {
      moreBtn.addEventListener('click', function (e) {
        e.stopPropagation();
        if (openMenu && openMenu.menu === moreMenu) closeMenus();
        else showMenu(moreMenu, moreBtn);
      });
    }

    // These parts are not in the backend yet.  They stay in the menu
    // so the shape of the program is visible, and they say so.
    var notReadyHandler = function () {
      closeMenus();
      notReady();
    };
    bind($('#btn-file'), 'click', notReadyHandler);
    bind($('#btn-terminal'), 'click', notReadyHandler);
    bind($('#btn-cloud-keyboard'), 'click', notReadyHandler);
    bind($('#btn-gamepad'), 'click', notReadyHandler);
    bind($('#btn-mapping'), 'click', notReadyHandler);
    bind($('#btn-privacy'), 'click', notReadyHandler);
    bind($('#btn-virtual-screen'), 'click', notReadyHandler);
    bind($('#btn-power'), 'click', notReadyHandler);
  }

  function wireSettingsPanel() {
    bind($('#settings-close'), 'click', closeDrawer);
    bind($('#settings-save-btn'), 'click', saveSettings);
    bind($('#settings-quit-btn'), 'click', function () {
      bridge.call('quit').catch(function () { /* the program is going away */ });
    });

    bind($('#settings-quality'), 'change', function (event) {
      var preset = QUALITY_PRESETS[event.target.value];
      if (!preset) return;
      $('#settings-bitrate').value = preset.bitrate;
      $('#bitrate-value').textContent = preset.bitrate + ' Mbps';
      $('#settings-framerate').value = preset.fps;
      $('#framerate-value').textContent = preset.fps + ' FPS';
    });

    bind($('#settings-bitrate'), 'input', function (event) {
      $('#bitrate-value').textContent = event.target.value + ' Mbps';
    });

    bind($('#settings-framerate'), 'input', function (event) {
      $('#framerate-value').textContent = event.target.value + ' FPS';
    });
  }

  function wireLogPanel() {
    bind($('#log-close'), 'click', closeDrawer);
    bind($('#log-clear'), 'click', clearLogs);
  }

  /* ==================================================================
   * Start
   * ================================================================== */

  function init() {
    loadSettings();
    hydrateIcons();
    applyI18n();
    updateLangLabel();
    writeSettingsForm();

    wireTopbar();
    wireMyDevice();
    wireDeviceList();
    wireAccessDialog();
    wireRemoteToolbar();
    wireSettingsPanel();
    wireLogPanel();
    wireRemoteInput();

    // The backend says in its own words that a part of the program is
    // not there yet.  The page has its own wording for it, so the
    // error is turned into that wording once, here.
    var nativeCall = bridge.call;
    bridge.call = function (command, payload) {
      return nativeCall(command, payload).catch(function (error) {
        if (error && error.code === 'notImplemented') {
          var ready = new Error(t('remote.notReady'));
          ready.code = 'notImplemented';
          throw ready;
        }
        throw error;
      });
    };

    bridge.on('connection', handleConnectionEvent);
    bridge.on('frame', renderFrame);
    bridge.on('host', handleHostEvent);
    bridge.on('settings', handleSettingsEvent);
    bridge.on('screens', function (payload) {
      state.screens = normaliseScreens((payload && payload.screens) || payload || []);
      renderScreenMenu();
    });
    bridge.on('toast', function (payload) {
      if (payload && payload.message) toast(payload.message, payload.kind || 'info');
    });
    bridge.on('disconnect', function () {
      state.connected = false;
      leaveRemoteView();
      toast(t('toast.disconnected'), 'info');
    });
    bridge.on('quit', function () {
      toast(t('toast.quit'), 'info');
    });
    bridge.on('log', function (entry) {
      if (!entry) return;
      state.logs.push(entry);
      renderLogs();
    });

    renderMyDevice();
    renderDevices();
    renderVerify();
    paintStatus();
    loadDevices();

    // The badge is only for when there is no backend: that is when
    // the page is opened in a browser on its own.
    var mockBadge = $('#footer-mock');
    if (mockBadge) mockBadge.hidden = bridge.isNative();

    // The settings the backend has on disk.  Everything it gives is
    // taken: the file is the truth, not the page.
    bridge.call('getSettings').then(function (settings) {
      if (settings && Object.keys(settings).length) handleSettingsEvent(settings);
    }).catch(function () { /* no settings yet */ });

    // What the tracker of this machine is doing.
    bridge.call('trackerInfo').then(function (info) {
      state.trackerInfo = info || null;
      renderMyDevice();
    }).catch(function () { /* no tracker */ });

    // Whether this machine is already sharing.
    bridge.call('hostStatus').then(function (result) {
      if (result && result.sharing) {
        state.sharing = true;
        state.hostState = 'waiting';
      }
      renderMyDevice();
    }).catch(function () { /* not sharing */ });

    // The version the program really is.
    bridge.call('appInfo').then(function (info) {
      if (!info) return;
      var version = info.name ? info.name + ' ' + info.version : info.version;
      setText($('#footer-version'), version);
      setText($('#settings-version'), version);
    }).catch(function () { /* no version */ });

    // The command line tool can open the window and go straight to a
    // device: "skycremote control --id something".
    var start = global.__SKYCREMOTE_START__;
    if (start && start.connectTo) requestConnect(start.connectTo);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})(typeof window !== 'undefined' ? window : globalThis);
