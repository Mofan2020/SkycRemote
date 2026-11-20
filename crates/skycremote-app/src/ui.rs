//! The window the user sees.
//!
//! The interface is plain HTML, CSS and JavaScript with no build step
//! and no framework.  It is folded into the binary so that the program
//! is one file to carry around.
//!
//! The page and the backend talk through one seam: the page calls
//! `window.__SKYCREMOTE_NATIVE__.request(command, payload)` and gets a
//! promise back, and the backend pushes events through
//! `window.__SKYCREMOTE_NATIVE__.emit(name, payload)`.  When that object
//! is not there the page falls back to its own mock, which is what
//! makes it possible to open the interface in a browser on its own.
//!
//! The window belongs to this thread, and the network belongs to
//! another one.  The two talk through channels, and this thread picks
//! the answers up every time it wakes.

use anyhow::{Context, Result};
use include_dir::include_dir;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebViewBuilder};

use crate::backend::{Backend, Out, UiRequest};

/// The interface files, folded into the binary.
static UI: include_dir::Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/ui");

/// How often the window looks for the answers the network has ready.
///
/// The window wakes up this often anyway, so there is nothing to gain
/// from a finer grain, and a coarser one would make the interface feel
/// slow.
const POLL: Duration = Duration::from_millis(30);

/// How the window is opened.
#[derive(Debug, Clone)]
pub struct Options {
    /// The width of the window in points.
    pub width: f64,
    /// The height of the window in points.
    pub height: f64,
    /// The title of the window.
    pub title: String,
    /// Whether this is a run to check that everything works.
    ///
    /// In that mode the window asks the backend for the list of
    /// screens once the page has loaded, prints what came back, and
    /// closes.  It is how the whole chain — window, page, bridge,
    /// backend, screen capture — is checked without a person in front
    /// of it.
    pub probe: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 820.0,
            title: "SkycRemote".to_string(),
            probe: false,
        }
    }
}

/// Read one of the interface files.
fn read(path: &str) -> String {
    match UI.get_file(path) {
        Some(file) => String::from_utf8_lossy(file.contents()).to_string(),
        None => String::new(),
    }
}

/// The page, with the stylesheet and the scripts folded into it.
///
/// Folding them in means there is one file to load, so there is no
/// path to get wrong and nothing to look for on disk.
fn build_page() -> String {
    let mut page = read("index.html");

    let stylesheet = read("style.css");
    page = page.replace(
        r#"<link rel="stylesheet" href="style.css">"#,
        &format!("<style>\n{stylesheet}\n</style>"),
    );

    for script in [
        "i18n.js",
        "components/icons.js",
        "components/bridge.js",
        "app.js",
    ] {
        let code = read(script);
        // A script that contains the closing tag would end the block
        // early, so the slash is put in front of it.
        let code = code.replace("</script>", "<\\/script>");
        page = page.replace(
            &format!(r#"<script src="{script}"></script>"#),
            &format!("<script>\n{code}\n</script>"),
        );
    }
    page
}

/// The script that gives the page its backend.
///
/// It runs before the page's own scripts, so the page finds the
/// backend there and uses it instead of its mock.
fn bootstrap() -> String {
    r#"
(function () {
  'use strict';

  var pending = {};
  var nextId = 1;

  window.__SKYCREMOTE_NATIVE__ = {
    // Ask the backend to do something.  The answer comes back later,
    // through __skycremote_resolve__.
    request: function (command, payloadJson) {
      return new Promise(function (resolve, reject) {
        var id = nextId++;
        pending[id] = { resolve: resolve, reject: reject };
        window.ipc.postMessage(JSON.stringify({
          id: id,
          command: command,
          payload: payloadJson === undefined ? null : payloadJson
        }));
      });
    },

    // The backend calls this to push an event into the page.  The page
    // replaces it with its own version once it has loaded.
    emit: function () {}
  };

  // A mistake in the page is worth knowing about, so it is reported
  // to the backend instead of only sitting in the console.
  window.addEventListener('error', function (event) {
    try {
      window.__SKYCREMOTE_NATIVE__.request('__pageError', JSON.stringify({
        message: event.message,
        file: event.filename,
        line: event.lineno
      }));
    } catch (ignored) {}
  });

  window.__skycremote_resolve__ = function (id, ok, result) {
    var entry = pending[id];
    if (!entry) return;
    delete pending[id];
    if (ok) {
      entry.resolve(result === undefined ? null : result);
    } else {
      var message = result && result.message ? result.message : String(result);
      var error = new Error(message);
      if (result && result.code) error.code = result.code;
      if (result && result.feature) error.feature = result.feature;
      entry.reject(error);
    }
  };
})();
"#
    .to_string()
}

/// Open the window and run until it is closed.
pub fn run(options: Options) -> Result<()> {
    let event_loop = EventLoop::new();

    let window = WindowBuilder::new()
        .with_title(&options.title)
        .with_inner_size(tao::dpi::LogicalSize::new(options.width, options.height))
        .build(&event_loop)
        .context("the window could not be opened")?;

    // The commands the page sends come in here.  The channel never
    // blocks: the page must not wait for the window to answer.
    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    // The answers and the events go back this way.
    let (out_tx, out_rx) = std::sync::mpsc::channel::<Out>();

    // The page says when it has finished loading.  The probe needs
    // that, because it must not ask the backend anything before the
    // page is ready to hear the answer.
    let page_loaded = Arc::new(AtomicBool::new(false));
    let loaded_flag = Arc::clone(&page_loaded);

    let builder = WebViewBuilder::new()
        .with_initialization_script(&bootstrap())
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, PageLoadEvent::Finished) {
                loaded_flag.store(true, Ordering::SeqCst);
            }
        })
        .with_ipc_handler(move |request| {
            if let Some(ui_request) = UiRequest::parse(&request.body()) {
                let _ = command_tx.send(ui_request);
            }
        });

    let webview = builder
        .build_as_child(&window)
        .context("the interface could not be loaded")?;

    // The network runs on a thread of its own: the thread that draws
    // the window must not sit waiting for a connection.
    let runtime = tokio::runtime::Runtime::new().context("the network could not be started")?;
    std::thread::spawn(move || {
        runtime.block_on(async move {
            Backend::new(out_tx).run(command_rx).await;
        });
    });

    webview
        .load_html(&build_page())
        .context("the interface could not be shown")?;

    let probe = options.probe;
    let mut probe_asked = false;

    event_loop.run(move |event, _, control_flow| {
        // The window wakes up now and then to hand the answers over.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL);

        // The probe asks once, as soon as the page can hear the answer.
        if probe && !probe_asked && page_loaded.load(Ordering::SeqCst) {
            probe_asked = true;
            eprintln!("PROBE: 頁面已加載，開始自檢");
            let _ = webview.evaluate_script(
                "window.__SKYCREMOTE_NATIVE__.request('listScreens', '{}')",
            );
        }

        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
            return;
        }

        loop {
            match out_rx.try_recv() {
                Ok(Out::Reply { id, ok, value }) => {
                    let script = format!(
                        "window.__skycremote_resolve__({}, {}, {})",
                        id, ok, value
                    );
                    let _ = webview.evaluate_script(&script);
                    if probe {
                        eprintln!("PROBE: 回覆 id={} ok={} {}", id, ok, value);
                        *control_flow = ControlFlow::Exit;
                    }
                }
                Ok(Out::Event { name, payload }) => {
                    if probe {
                        eprintln!("PROBE: 事件 {} {}", name, payload);
                    }
                    let script = format!(
                        "window.__SKYCREMOTE_NATIVE__.emit({}, {})",
                        serde_json::to_string(&name).unwrap_or_else(|_| "\"\"".to_string()),
                        payload
                    );
                    let _ = webview.evaluate_script(&script);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    });
    Ok(())
}
