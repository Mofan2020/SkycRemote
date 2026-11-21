/*!
 * SkycRemote UI — backend bridge
 *
 * ---------------------------------------------------------------------
 * THIS IS A MOCK.  The Rust side does not exist yet.
 * ---------------------------------------------------------------------
 * Every method below returns made-up data so the interface can be
 * exercised and reviewed on its own.  Nothing here talks to a network,
 * a capture device, or a real remote machine.
 *
 * The seam for the real backend:
 *
 *   Rust injects a native object before the page scripts run:
 *
 *       window.__SKYCREMOTE_NATIVE__ = {
 *           request: function (command, payloadJson) { ... },  // returns a Promise
 *           emit:   function (eventName, payloadJson) { ... }   // Rust -> JS
 *       };
 *
 *   Then `Bridge.call()` forwards to `__SKYCREMOTE_NATIVE__.request()`
 *   and `Bridge.on()` listens for `__SKYCREMOTE_NATIVE__.emit()`.
 *   When that global is absent the mock is used instead, which is
 *   exactly the current state of the project.
 *
 * The shapes below intentionally mirror `skycremote-core/src/protocol.rs`
 * so the wire format does not have to change when the backend lands:
 * `Message`, `InputEvent` and `ScreenInfo` are the Rust names.
 */
(function (global) {
  'use strict';

  var NS = (global.SkycUI = global.SkycUI || {});

  /* ------------------------------------------------------------------
   * Fake data
   * ---------------------------------------------------------------- */

  var FAKE_DEVICES = [
    { id: '4821-9930-AX7', name: 'Studio Mac Pro', online: true },
    { id: '1044-7721-KQ2', name: 'Windows Workstation', online: true },
    { id: '7730-2218-LM4', name: 'Office iMac', online: false }
  ];

  // The code the other side asks for.  It is shown in the dialog so
  // that the demo can be walked through.
  var FAKE_PEER_CODE = '1234';

  var FAKE_SETTINGS = {
    language: 'zh',
    theme: 'dark',
    tracker: 'http://127.0.0.1:21115',
    deviceId: 'demo-mac-4821',
    deviceName: '我的 Mac',
    quality: 'auto',
    bitrate: 8,
    framerate: 30,
    accessCode: '8899',
    savedCodes: { '1044-7721-KQ2': '2468' }
  };

  var FAKE_LOGS = [
    { time: Date.now() - 60000, level: 'info', message: '程序起来了' },
    { time: Date.now() - 30000, level: 'info', message: 'Tracker 在 http://127.0.0.1:21115' },
    { time: Date.now() - 5000, level: 'warning', message: '还没有设置设备 ID' }
  ];

  var FAKE_SCREENS = [
    { index: 0, name: 'Built-in Retina Display', width: 2560, height: 1440, scaleFactor: 2, primary: true },
    { index: 1, name: 'External Display', width: 1920, height: 1080, scaleFactor: 1, primary: false }
  ];

  var FAKE_FILES = {
    '/': [
      { name: 'Desktop', isDir: true, size: 0, modified: '2026-11-19T10:20:00Z' },
      { name: 'Documents', isDir: true, size: 0, modified: '2026-11-19T08:02:00Z' },
      { name: 'Downloads', isDir: true, size: 0, modified: '2026-11-18T22:41:00Z' },
      { name: 'Pictures', isDir: true, size: 0, modified: '2026-11-17T14:15:00Z' },
      { name: 'report-2026-Q4.pdf', isDir: false, size: 2483712, modified: '2026-11-19T06:30:00Z' },
      { name: 'demo-recording.mp4', isDir: false, size: 187654321, modified: '2026-11-16T19:44:00Z' },
      { name: 'notes.txt', isDir: false, size: 4312, modified: '2026-11-15T11:08:00Z' }
    ],
    '/Desktop': [
      { name: 'project-plan.md', isDir: false, size: 8192, modified: '2026-11-19T10:19:00Z' },
      { name: 'screenshots', isDir: true, size: 0, modified: '2026-11-18T12:00:00Z' }
    ],
    '/Documents': [
      { name: 'contract.pdf', isDir: false, size: 1048576, modified: '2026-11-12T09:00:00Z' },
      { name: 'budget.xlsx', isDir: false, size: 262144, modified: '2026-11-10T15:30:00Z' }
    ],
    '/Downloads': [
      { name: 'installer.dmg', isDir: false, size: 104857600, modified: '2026-11-19T07:12:00Z' },
      { name: 'archive.zip', isDir: false, size: 52428800, modified: '2026-11-18T20:00:00Z' }
    ],
    '/Pictures': [
      { name: 'sunset.png', isDir: false, size: 4194304, modified: '2026-11-09T18:20:00Z' }
    ],
    '/Desktop/screenshots': []
  };

  /* ------------------------------------------------------------------
   * Transport
   * ---------------------------------------------------------------- */

  var listeners = Object.create(null);
  var native = global.__SKYCREMOTE_NATIVE__ || null;

  function emitLocal(eventName, payload) {
    var handlers = listeners[eventName];
    if (!handlers) return;
    for (var i = 0; i < handlers.length; i++) {
      try {
        handlers[i](payload);
      } catch (err) {
        if (global.console && console.error) console.error('[skycremote] handler failed for ' + eventName, err);
      }
    }
  }

  /** Rust calls this to push an event into the UI. */
  function emitNative(eventName, payloadJson) {
    var payload = payloadJson;
    if (typeof payloadJson === 'string') {
      try { payload = JSON.parse(payloadJson); } catch (e) { payload = payloadJson; }
    }
    emitLocal(eventName, payload);
  }

  // When Rust is present it owns the event channel.  Expose the same
  // entry point under both names so the app code does not care.
  if (native && typeof native.emit === 'function') {
    native.emit = (function (original) {
      return function (name, payload) { original(name, payload); };
    })(native.emit);
  }

  /**
   * Call a backend command.
   * @param {string} command  e.g. "connect", "listScreens"
   * @param {object} [payload]
   * @returns {Promise<any>}
   */
  function call(command, payload) {
    if (native && typeof native.request === 'function') {
      return Promise.resolve(native.request(command, JSON.stringify(payload || {})))
        .then(function (result) {
          if (command === 'listScreens' && Array.isArray(result)) {
            return result.map(normaliseScreen);
          }
          return result;
        });
    }
    return mockCall(command, payload || {});
  }

  /** Subscribe to a backend event. */
  function on(eventName, handler) {
    if (!listeners[eventName]) listeners[eventName] = [];
    listeners[eventName].push(handler);
    return function off() {
      var arr = listeners[eventName];
      if (!arr) return;
      var i = arr.indexOf(handler);
      if (i !== -1) arr.splice(i, 1);
    };
  }

  /**
   * Normalise a screen entry.
   *
   * Rust's serde emits `scale_factor` (snake_case, as in
   * `skycremote-core/src/protocol.rs`), while the UI code uses
   * `scaleFactor`. Accept both so the bridge does not have to care
   * which side produced the entry.
   */
  function normaliseScreen(screen) {
    if (!screen) return null;
    var scale = screen.scaleFactor !== undefined ? screen.scaleFactor : screen.scale_factor;
    return {
      index: screen.index,
      name: screen.name,
      width: screen.width,
      height: screen.height,
      scaleFactor: scale === undefined ? 1 : scale,
      primary: !!screen.primary,
      virtual: !!screen.virtual
    };
  }

  /**
   * Normalise one entry of a folder listing.
   *
   * Rust's serde emits `is_dir` (snake_case, as in
   * `skycremote-core/src/protocol.rs`), while the UI code uses
   * `isDir`.  Accept both so the bridge does not have to care which
   * side produced the entry.
   */
  function normaliseFileEntry(entry) {
    if (!entry) return null;
    var isDir = entry.isDir !== undefined ? entry.isDir : entry.is_dir;
    return {
      name: entry.name || '',
      isDir: !!isDir,
      size: Number(entry.size) || 0
    };
  }

  /* ------------------------------------------------------------------
   * Mock implementation
   * ---------------------------------------------------------------- */

  // Every transfer the mock hands out gets a token, the way the
  // real backend names one transfer at a time.
  var mockToken = 0;
  var mockUploads = {};

  function nextToken() {
    mockToken += 1;
    return mockToken;
  }

  var mockState = {
    connected: false,
    sharing: false,
    screen: 0,
    virtualScreen: false,
    privacyScreen: false,
    latency: 18,
    bitrate: 8.4,
    fps: 60,
    settings: JSON.parse(JSON.stringify(FAKE_SETTINGS)),
    logs: FAKE_LOGS.slice()
  };

  /** An error the way the backend sends one. */
  function fail(code, message) {
    var error = new Error(message);
    error.code = code;
    return Promise.reject(error);
  }

  function delay(ms, value) {
    return new Promise(function (resolve) {
      setTimeout(function () { resolve(value); }, ms);
    });
  }

  function mockCall(command, payload) {
    switch (command) {
      case 'connect': {
        var code = (payload.accessCode || '').trim();
        if (!code) return fail('noAccessCode', '对面没有设访问口令');
        if (code !== FAKE_PEER_CODE) return fail('accessDenied', '访问口令不对');
        mockState.connected = true;
        emitLocal('connection', { state: 'connecting', deviceId: payload.deviceId });
        setTimeout(function () {
          if (!mockState.connected) return;
          emitLocal('connection', {
            state: 'connected',
            deviceId: payload.deviceId,
            peer: '192.168.1.31:21116',
            latencyMs: 18,
            verifyCode: '482913'
          });
        }, 200);
        return delay(360, { ok: true, deviceId: payload.deviceId });
      }

      case 'disconnect':
        mockState.connected = false;
        return delay(80, { ok: true });

      case 'startHost':
        mockState.sharing = true;
        emitLocal('host', { state: 'waiting' });
        setTimeout(function () {
          if (!mockState.sharing) return;
          emitLocal('host', { state: 'sharing', peer: '192.168.1.31:21116', verifyCode: '482913' });
        }, 900);
        return delay(120, { ok: true, deviceId: mockState.settings.deviceId });

      case 'stopHost':
        mockState.sharing = false;
        emitLocal('host', { state: 'stopped' });
        return delay(80, { ok: true });

      case 'hostStatus':
        return delay(40, { sharing: mockState.sharing });

      case 'listScreens':
        return delay(120, FAKE_SCREENS.slice());

      case 'switchScreen':
        mockState.screen = payload.index;
        return delay(60, { ok: true, index: payload.index });

      case 'listDevices':
        return delay(140, FAKE_DEVICES.slice());

      case 'listFiles': {
        var wanted = payload.path || '';
        var entries = FAKE_FILES[wanted] ||
          FAKE_FILES['/' + wanted] ||
          (wanted ? [] : FAKE_FILES['/']);
        var listToken = nextToken();
        setTimeout(function () {
          emitLocal('files', { token: listToken, path: wanted, entries: entries });
        }, 220);
        return delay(180, { ok: true, token: listToken });
      }

      case 'sendInput':
        // Input is fire-and-forget.  A real backend would ship the
        // `InputEvent` enum to the peer.
        return delay(0, { ok: true });

      case 'sendClipboard':
        setTimeout(function () {
          emitLocal('clipboard', { text: payload.text || '' });
        }, 60);
        return delay(40, { ok: true });

      case 'powerAction':
        return delay(120, { ok: true, action: payload.action });

      case 'getSettings':
        return delay(60, JSON.parse(JSON.stringify(mockState.settings)));

      case 'saveSettings':
        mockState.settings = JSON.parse(JSON.stringify(payload || {}));
        return delay(80, { ok: true, settings: mockState.settings, applied: [] });

      case 'getLogs':
        return delay(40, { entries: mockState.logs.slice() });

      case 'clearLogs':
        mockState.logs = [];
        return delay(40, { ok: true });

      case 'trackerInfo':
        return delay(40, {
          ok: true,
          url: mockState.settings.tracker,
          lanUrl: 'http://192.168.1.20:21115',
          local: true,
          running: true
        });

      case 'appInfo':
        return delay(20, { name: 'SkycRemote', version: '0.6.0' });

      case 'log':
        mockState.logs.push({
          time: Date.now(),
          level: payload.level || 'info',
          message: payload.message || ''
        });
        return delay(10, { ok: true });

      case 'quit':
        return delay(20, { ok: true });

      case 'uploadStart': {
        var uploadToken = nextToken();
        mockUploads[uploadToken] = {
          name: payload.name || '',
          size: Number(payload.size) || 0,
          sent: 0
        };
        return delay(80, { ok: true, token: uploadToken });
      }

      case 'uploadChunk': {
        var upload = mockUploads[payload.token];
        if (upload) {
          // The chunk is base64, so three bytes make four characters.
          upload.sent += Math.round(String(payload.data || '').length * 3 / 4);
        }
        return delay(20, { ok: true });
      }

      case 'uploadEnd': {
        var finished = mockUploads[payload.token];
        if (finished) {
          emitLocal('uploadProgress', {
            token: payload.token,
            sent: finished.size,
            size: finished.size
          });
          emitLocal('uploadDone', { token: payload.token, name: finished.name });
          delete mockUploads[payload.token];
        }
        return delay(40, { ok: true });
      }

      case 'downloadFile': {
        var fileToken = nextToken();
        var fileName = String(payload.path || '').split('/').pop() || 'file';
        var total = 2 * 1024 * 1024;
        var got = 0;
        setTimeout(function () {
          var timer = setInterval(function () {
            got += total / 8;
            if (got >= total) {
              clearInterval(timer);
              emitLocal('fileDone', {
                token: fileToken,
                name: fileName,
                size: total,
                path: '/Users/skyc/Downloads/' + fileName
              });
              return;
            }
            emitLocal('fileProgress', {
              token: fileToken,
              received: Math.round(got),
              size: total
            });
          }, 120);
        }, 120);
        return delay(100, { ok: true, token: fileToken });
      }

      case 'togglePrivacyScreen': {
        mockState.privacyScreen = !!payload.enabled;
        emitLocal('privacy', { enabled: mockState.privacyScreen });
        return delay(80, { ok: true, enabled: mockState.privacyScreen });
      }

      case 'cancelTransfer':
        return delay(40, { ok: true });

      case 'terminalExec':
        return delay(120, { ok: true });

      default:
        return delay(30, { ok: true, mock: true, command: command });
    }
  }

  /* ------------------------------------------------------------------
   * Synthetic frame source
   *
   * A real backend pushes encoded video frames; the browser cannot
   * decode H.264 without a demuxer, so the mock paints a placeholder
   * scene straight onto the canvas instead.  It is deliberately
   * obvious that it is a placeholder.
   * ---------------------------------------------------------------- */

  var frameTimer = null;
  var frameStart = 0;

  function startFakeFrames(canvas) {
    if (frameTimer) return;
    frameStart = Date.now();
    var ctx = canvas.getContext('2d');
    if (!ctx) return;

    function paint() {
      var w = canvas.width;
      var h = canvas.height;
      var t = (Date.now() - frameStart) / 1000;

      // background
      var g = ctx.createLinearGradient(0, 0, w, h);
      g.addColorStop(0, '#101826');
      g.addColorStop(1, '#1b2740');
      ctx.fillStyle = g;
      ctx.fillRect(0, 0, w, h);

      // moving accent band so motion is visible
      var x = (Math.sin(t * 0.6) * 0.5 + 0.5) * w;
      var band = ctx.createRadialGradient(x, h * 0.45, 10, x, h * 0.45, Math.max(w, h) * 0.45);
      band.addColorStop(0, 'rgba(88, 150, 255, 0.35)');
      band.addColorStop(1, 'rgba(88, 150, 255, 0)');
      ctx.fillStyle = band;
      ctx.fillRect(0, 0, w, h);

      // fake window chrome
      var pw = w * 0.46;
      var ph = h * 0.52;
      var px = (w - pw) / 2 + Math.cos(t * 0.4) * w * 0.02;
      var py = (h - ph) / 2;
      ctx.fillStyle = 'rgba(255,255,255,0.07)';
      roundRect(ctx, px, py, pw, ph, 18);
      ctx.fill();

      ctx.fillStyle = 'rgba(255,255,255,0.16)';
      roundRect(ctx, px, py, pw, 46, 18);
      ctx.fill();
      ctx.fillRect(px, py + 26, pw, 20);

      ctx.fillStyle = 'rgba(255,255,255,0.55)';
      ctx.font = '500 22px -apple-system, system-ui, sans-serif';
      ctx.fillText('SkycRemote — mock frame', px + 24, py + 30);

      ctx.fillStyle = 'rgba(255,255,255,0.30)';
      ctx.font = '400 18px -apple-system, system-ui, sans-serif';
      ctx.fillText('No video decoder is connected yet.', px + 24, py + 100);
      ctx.fillText('This canvas is painted by the mock bridge.', px + 24, py + 132);

      // clock so the refresh rate is obvious
      ctx.fillStyle = 'rgba(255,255,255,0.45)';
      ctx.font = '300 64px -apple-system, system-ui, sans-serif';
      var stamp = new Date();
      ctx.fillText(
        String(stamp.getHours()).padStart(2, '0') + ':' +
        String(stamp.getMinutes()).padStart(2, '0') + ':' +
        String(stamp.getSeconds()).padStart(2, '0'),
        (w - 200) / 2, h * 0.86
      );

      // fake latency sparkline
      ctx.strokeStyle = 'rgba(120, 190, 255, 0.5)';
      ctx.lineWidth = 2;
      ctx.beginPath();
      for (var i = 0; i <= 60; i++) {
        var px2 = (i / 60) * w;
        var py2 = h - 60 + Math.sin(i * 0.35 + t * 2) * 18 * Math.sin(t * 0.5 + i * 0.1);
        if (i === 0) ctx.moveTo(px2, py2); else ctx.lineTo(px2, py2);
      }
      ctx.stroke();

      emitLocal('frame', { width: w, height: h, mock: true });
    }

    paint();
    frameTimer = setInterval(paint, 1000 / 30);
  }

  function stopFakeFrames() {
    if (frameTimer) {
      clearInterval(frameTimer);
      frameTimer = null;
    }
  }

  function roundRect(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  /* ------------------------------------------------------------------
   * Public surface
   * ---------------------------------------------------------------- */

  NS.bridge = {
    /** True when a real Rust backend is attached. */
    isNative: function () { return !!(native && typeof native.request === 'function'); },

    call: call,
    on: on,
    emit: emitNative,
    normaliseScreen: normaliseScreen,
    normaliseFileEntry: normaliseFileEntry,

    // mock-only helpers, used by the UI to simulate a session
    mock: {
      state: mockState,
      devices: FAKE_DEVICES,
      screens: FAKE_SCREENS,
      files: FAKE_FILES,
      startFrames: startFakeFrames,
      stopFrames: stopFakeFrames
    }
  };
})(typeof window !== 'undefined' ? window : globalThis);
