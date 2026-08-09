//! A browser, driven over its own protocol.
//!
//! [`docs/21-tests-in-beck-and-proof.md`](../../../../../docs/21-tests-in-beck-and-proof.md) §21.4:
//! "§21.2's cross-boundary tests run the tiers co-located. That proves what the boundary *means*,
//! not that a particular browser renders it. Phase 3's Mode B will need a browser in CI." This is
//! that browser, and everything about it is chosen so that having one costs the project nothing it
//! was not already paying:
//!
//! * **No new dependency.** The Chrome DevTools Protocol is JSON over a websocket, and
//!   `tokio-tungstenite` is already a dev-dependency here — it is what
//!   [`crate::support::socket`] drives `beck_rt::session::run` with. A browser-automation library
//!   would be a large dependency for one suite, and this is about 200 lines.
//! * **No Node, no npm, no driver binary.** Chromium is launched directly and told to write its
//!   own debugging port into its profile directory, which is how its endpoint is discovered
//!   without parsing a log.
//! * **It skips loudly.** A machine without Chromium runs everything else;
//!   `BECK_REQUIRE_BROWSER=1` forbids the skip, which is what CI sets. The convention every
//!   environment-dependent suite in this workspace follows.
//!
//! What it deliberately does *not* do is drive the page like a user with a mouse: a click here is
//! `element.click()` and a keypress is a dispatched `KeyboardEvent`, both of which go through the
//! real listeners the real residue installed. What is under test is Beck's JavaScript, not
//! Chromium's hit-testing.

#![allow(dead_code)] // each test binary uses the half of this it needs

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Where to find Chromium: `BECK_CHROME`, then the two paths a Playwright install uses, then the
/// names a distribution package installs.
fn chromium() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("BECK_CHROME") {
        let path = PathBuf::from(named);
        return path.is_file().then_some(path);
    }
    let root = std::env::var("PLAYWRIGHT_BROWSERS_PATH").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !root.is_empty() {
        // The directories carry a build number, so they are globbed rather than named.
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                candidates.push(entry.path().join("chrome-linux/headless_shell"));
                candidates.push(entry.path().join("chrome-linux/chrome"));
            }
        }
    }
    candidates.extend(
        ["chromium", "chromium-browser", "google-chrome", "chrome"]
            .into_iter()
            .filter_map(which),
    );
    candidates.into_iter().find(|p| p.is_file())
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

/// Say why a suite is not running, or fail if the environment promised one.
///
/// Returns `None` when there is no browser and the skip is allowed, so a caller's `let Some(..)
/// else { return }` reads as the skip it is.
pub fn available() -> Option<PathBuf> {
    let found = chromium();
    if found.is_none() {
        assert!(
            std::env::var("BECK_REQUIRE_BROWSER").as_deref() != Ok("1"),
            "BECK_REQUIRE_BROWSER=1 but no Chromium was found (set BECK_CHROME, or install one)"
        );
        eprintln!(
            "skipped: no Chromium on this machine. Set BECK_CHROME to one, or \
             BECK_REQUIRE_BROWSER=1 to forbid this skip."
        );
    }
    found
}

pub struct Browser {
    child: Child,
    socket: Socket,
    profile: PathBuf,
    next_id: i64,
    /// The page this browser last opened, closed when the next one is.
    open_target: Option<String>,
}

/// The runtime every browser test runs on.
///
/// `#[tokio::test]` builds a runtime *per test*, and a `tokio` socket or child process belongs to
/// the runtime that created it — so a browser shared between tests is a connection used from a
/// runtime that does not own it, which hangs rather than failing. One runtime for the suite, and
/// [`browser_test!`] is what puts each test on it.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime for the browser suite")
    })
}

/// A test that needs the browser: an ordinary `#[test]`, run on [`runtime`].
#[macro_export]
macro_rules! browser_test {
    ($(#[$meta:meta])* async fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            support::browser::runtime().block_on(async $body);
        }
    };
}

/// The one browser this suite runs, and the lock that makes its tests serial.
///
/// Launching one per test meant six headless Chromiums on a machine that was also running the rest
/// of `cargo test --workspace` — the debug measurement suites among them — and under that load a
/// DevTools command went unanswered for thirty seconds and the suite failed. It passed run on its
/// own, which is the definition of the gate `docs/13` §13.7 refuses to have: "a gate that flakes is
/// worse than no gate".
///
/// So: one browser, and a lock held for the length of a test. Browser tests are heavy and there is
/// nothing to be gained by overlapping them. Their *pages* stay isolated from each other for free —
/// each test serves on a port of its own, and a different port is a different origin, so no two
/// share a `localStorage`.
static BROWSER: tokio::sync::OnceCell<tokio::sync::Mutex<Browser>> =
    tokio::sync::OnceCell::const_new();

/// The browser, locked. `None` when there is none to be had — see [`available`].
pub async fn shared() -> Option<tokio::sync::MutexGuard<'static, Browser>> {
    let binary = available()?;
    Some(
        BROWSER
            .get_or_init(|| async { tokio::sync::Mutex::new(Browser::launch(&binary).await) })
            .await
            .lock()
            .await,
    )
}

impl Browser {
    /// Launch headless Chromium and connect to its browser endpoint.
    pub async fn launch(binary: &PathBuf) -> Browser {
        let profile = std::env::temp_dir().join(format!(
            "beck-browser-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile).expect("a profile directory");

        let child = Command::new(binary)
            .args([
                "--headless=new",
                // Root in a container, and no /dev/shm worth the name. Neither is a property of
                // what is under test, and both are how every CI runs a browser.
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--disable-gpu",
                "--remote-debugging-port=0",
                "about:blank",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("chromium starts");

        // Chromium writes `<port>\n<browser ws path>` here once it is listening. Reading the file
        // rather than its stderr is what makes this independent of its logging.
        let stamp = profile.join("DevToolsActivePort");
        let deadline = Instant::now() + Duration::from_secs(30);
        let endpoint = loop {
            if let Ok(text) = std::fs::read_to_string(&stamp) {
                let mut lines = text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                    break format!("ws://127.0.0.1:{port}{path}");
                }
            }
            assert!(Instant::now() < deadline, "chromium never opened a port");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let (socket, _) = connect_async(&endpoint).await.expect("devtools connects");
        Browser {
            child,
            socket,
            profile,
            next_id: 0,
            open_target: None,
        }
    }

    /// One command, and the reply to it.
    ///
    /// Events arrive on the same socket interleaved with replies, so this reads until it sees the
    /// id it sent. Everything else is dropped: this suite polls for the state it wants rather than
    /// subscribing, which is longer to write but immune to the ordering of load events.
    async fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let mut frame = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            frame["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .expect("sends a command");

        // Generous, because a machine running this suite is running the rest of the workspace too
        // and a browser is the first thing to be starved of a core. It is still bounded: a command
        // that never answers is a bug, and hanging on it would be a suite that never fails.
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            assert!(Instant::now() < deadline, "`{method}` never answered");
            let Some(Ok(Message::Text(text))) = self.socket.next().await else {
                continue;
            };
            let message: Value = serde_json::from_str(&text).expect("devtools speaks JSON");
            if message.get("id").and_then(|i| i.as_i64()) == Some(id) {
                if let Some(error) = message.get("error") {
                    panic!("`{method}` failed: {error}");
                }
                return message.get("result").cloned().unwrap_or(Value::Null);
            }
        }
    }

    /// Open a page on `url` and attach to it.
    pub async fn open(&mut self, url: &str) -> Page {
        // The previous test's page first. A shared browser makes leaving them open a real cost:
        // each one keeps a script running, a socket dialling a server that has stopped, and a
        // service worker registered — and the next test would be measuring a machine carrying all
        // of them.
        if let Some(previous) = self.open_target.take() {
            self.call("Target.closeTarget", json!({"targetId": previous}), None)
                .await;
        }
        let target = self
            .call("Target.createTarget", json!({"url": "about:blank"}), None)
            .await["targetId"]
            .as_str()
            .expect("a target id")
            .to_string();
        let session = self
            .call(
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
                None,
            )
            .await["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string();

        self.open_target = Some(target);
        let mut page = Page {
            session,
            url: url.to_string(),
        };
        self.call("Page.enable", json!({}), Some(&page.session))
            .await;
        self.call("Runtime.enable", json!({}), Some(&page.session))
            .await;
        // Installed before any document runs, so it catches the failures that happen while the
        // page is still loading — which is where a module that will not instantiate fails. Without
        // this a browser test says "the page never changed" and not *why*, and the why is the only
        // thing the suite exists to find out.
        self.call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": CAPTURE}),
            Some(&page.session),
        )
        .await;
        page.navigate(self, url).await;
        page
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // `kill_on_drop` handles the process; the profile is ours to remove.
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// Everything the browser would have written to a console nobody is reading.
const CAPTURE: &str = r#"
(() => {
  window.__beckLog = [];
  const say = (kind, args) => window.__beckLog.push(kind + ": " + args.map(String).join(" "));
  for (const kind of ["error", "warn", "log"]) {
    const was = console[kind];
    console[kind] = (...args) => { say(kind, args); was.apply(console, args); };
  }
  window.addEventListener("error", (e) => say("uncaught", [e.message, e.filename + ":" + e.lineno]));
  window.addEventListener("unhandledrejection", (e) => say("unhandled", [e.reason]));
  window.addEventListener("beck:error", (e) => say("beck:error", [JSON.stringify(e.detail)]), true);
})();
"#;

pub struct Page {
    session: String,
    pub url: String,
}

impl Page {
    pub async fn navigate(&mut self, browser: &mut Browser, url: &str) {
        browser
            .call("Page.navigate", json!({"url": url}), Some(&self.session))
            .await;
        self.url = url.to_string();
        self.wait_for(browser, "document.readyState === 'complete'")
            .await;
    }

    /// Evaluate an expression and bring the value back.
    pub async fn eval(&self, browser: &mut Browser, js: &str) -> Value {
        let out = browser
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": js,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
                Some(&self.session),
            )
            .await;
        if let Some(details) = out.get("exceptionDetails") {
            panic!("`{js}` threw: {details}");
        }
        out.get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub async fn text(&self, browser: &mut Browser, js: &str) -> String {
        self.eval(browser, js)
            .await
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Cut the page off from the network, or reconnect it.
    ///
    /// Chromium's own emulation rather than a proxy or a firewall rule: what is under test is what
    /// the client does when `fetch` and `WebSocket` fail, and this is the switch that makes them.
    pub async fn offline(&self, browser: &mut Browser, cut: bool) {
        browser
            .call("Network.enable", json!({}), Some(&self.session))
            .await;
        browser
            .call(
                "Network.emulateNetworkConditions",
                json!({
                    "offline": cut,
                    "latency": 0,
                    "downloadThroughput": if cut { 0 } else { -1 },
                    "uploadThroughput": if cut { 0 } else { -1 },
                }),
                Some(&self.session),
            )
            .await;
    }

    /// Poll until the expression is truthy, or fail saying what it was.
    ///
    /// A browser is asynchronous in ways nothing here controls — a socket opening, a module
    /// instantiating, a frame arriving — so every assertion about the page is a *wait* with a
    /// deadline rather than a read after a sleep. A sleep long enough to be reliable is long
    /// enough to make the suite slow, and one short enough to be fast is a flake.
    pub async fn wait_for(&self, browser: &mut Browser, js: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let value = self.eval(browser, js).await;
            if truthy(&value) {
                return;
            }
            if Instant::now() >= deadline {
                let dom = self
                    .text(
                        browser,
                        "(document.getElementById('b-root') || document.body).innerHTML",
                    )
                    .await;
                let log = self
                    .text(browser, "JSON.stringify(window.__beckLog || [], null, 1)")
                    .await;
                panic!(
                    "waited 20s for `{js}`, last value {value}\n\
                     --- the browser said ---\n{log}\n--- the page ---\n{dom}"
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        _ => true,
    }
}
