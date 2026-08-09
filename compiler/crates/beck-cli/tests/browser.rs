//! Both rendering modes, in a real browser.
//!
//! Everything else in this directory tests Beck against Beck. This is the one suite that runs the
//! **JavaScript** — `beck-patch.js`, `beck-thin.js`, `beck-mode-b.js` and the WebAssembly kernel —
//! in the program they were written for, which is a browser and not a harness.
//!
//! [`docs/94-mode-b-report.md`](../../../../docs/94-mode-b-report.md) §94.8 listed "no browser has
//! run it" as the largest hole in Mode B, and §94.7 says what the hole had already cost: a served
//! document that never contained the element both clients open by looking for, and a thin client
//! that had therefore returned immediately in every browser since Phase 1, with every test in the
//! workspace passing. A suite that cannot execute the residue cannot see that class of defect at
//! all.
//!
//! It skips loudly without Chromium; `BECK_REQUIRE_BROWSER=1` forbids the skip.

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beck_core::{Placed, Value};

mod support;
use support::browser::{self, Browser, Page};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("the compiler directory")
}

fn example(name: &str) -> Placed {
    let path = root().join("examples").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("examples/{name}"));
    let (placed, diags, map) = beck_core::compile_str(path.to_str().expect("utf-8"), &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

/// A port nothing is listening on, by asking the kernel for one and letting it go.
fn free_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let addr = listener.local_addr().expect("its address");
    drop(listener);
    addr
}

/// Point the server at the kernel this workspace builds, once.
///
/// `BECK_KERNEL` is the operator's interface (`beck_rt::http::kernel_path`) and it is process-wide,
/// so it is set here rather than per test: every test in this binary wants the same value, and the
/// alternative — a runtime API that exists only for a test — would be a worse seam than an
/// environment variable that already exists for a real reason.
fn point_at_the_kernel() -> bool {
    static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let module = root().join("target/wasm32-unknown-unknown/release/beck_wasm.wasm");
        if !module.is_file() {
            return false;
        }
        std::env::set_var("BECK_KERNEL", &module);
        true
    })
}

/// A running application, on a port of its own.
struct Serving {
    app: Arc<beck_rt::App>,
    addr: SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl Serving {
    async fn start(placed: Placed) -> Serving {
        let backend = beck_eval::backend(&placed);
        let app = beck_rt::App::start(
            beck_rt::Runtime::new(placed, backend).expect("prepares"),
            Arc::new(beck_rt::MemoryLog::new()),
            beck_rt::AppConfig::default(),
        )
        .await
        .expect("the app starts");

        let addr = free_port();
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        let serving = app.clone();
        tokio::spawn(async move {
            let _ = beck_rt::http::serve(serving, addr, rx).await;
        });
        Serving {
            app,
            addr,
            shutdown,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/?actor=ana", self.addr)
    }

    /// What the server itself would render for this actor, as markup.
    async fn rendered(&self, actor: &str) -> String {
        self.app.render(actor).await.expect("renders").render()
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// The DOM, as the browser serialises the frame root's children.
async fn dom(page: &Page, browser: &mut Browser) -> String {
    page.text(browser, "document.getElementById('b-root').innerHTML")
        .await
}

// --------------------------------------------------------------- Mode A

/// The thin client connects, renders and applies a patch — in a browser, for the first time.
///
/// The `contains` assertions are about *the DOM after a round trip*: the card is not in the
/// server-rendered document, so its appearance is a patch this browser received over a websocket
/// and applied with `beck-patch.js`.
#[tokio::test]
async fn mode_a_applies_the_servers_patches() {
    let Some(binary) = browser::available() else {
        return;
    };
    let serving = Serving::start(example("todo.beck")).await;
    let mut browser = Browser::launch(&binary).await;
    let page = browser.open(&serving.url()).await;

    // "Live" is the client's own signal, not a guess about how long a socket takes: `data-b-ready`
    // carries the mode's letter, and in Mode A it appears when the subscription is welcomed.
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'a'",
    )
    .await;
    page.eval(
        &mut browser,
        "window.__beckSeen = []; \
         document.addEventListener('beck:rejected', (e) => window.__beckSeen.push(e.detail));",
    )
    .await;

    // Type a todo and press Enter, through the declared handler rather than around it.
    page.eval(
        &mut browser,
        "(() => { const i = document.querySelector('input[data-b-enter]'); i.value = 'milk'; \
          i.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true})); })()",
    )
    .await;

    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('milk')",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('1 remaining')",
    )
    .await;
    assert_eq!(
        page.eval(&mut browser, "window.__beckSeen.length").await,
        0,
        "the server refused something"
    );

    // And the DOM is the page the server holds — not similar to it.
    assert_eq!(
        dom(&page, &mut browser).await,
        serving.rendered("ana").await,
        "the browser's DOM and the server's render disagree"
    );
}

// --------------------------------------------------------------- Mode B

/// The whole of Mode B, in the program it was written for.
///
/// Loads the kernel and the bundle over HTTP, renders locally from a data patch, applies a command
/// optimistically, and ends up with the page the server would have rendered.
#[tokio::test]
async fn mode_b_renders_in_the_browser_and_guesses_ahead_of_the_server() {
    let Some(binary) = browser::available() else {
        return;
    };
    if !point_at_the_kernel() {
        eprintln!(
            "skipped: no kernel to serve. Build it with \
             `cargo build -p beck-wasm --release --target wasm32-unknown-unknown`."
        );
        assert!(
            std::env::var("BECK_REQUIRE_WASM").as_deref() != Ok("1"),
            "BECK_REQUIRE_WASM=1 but there is no kernel to serve"
        );
        return;
    }

    let serving = Serving::start(example("board.beck")).await;
    let mut browser = Browser::launch(&binary).await;
    let page = browser.open(&serving.url()).await;

    // The document says which residue it loaded, and for a Mode B component that is the kernel.
    assert!(
        page.eval(
            &mut browser,
            "!!document.querySelector('script[src=\"/beck-mode-b.js\"]')"
        )
        .await
        .as_bool()
            == Some(true),
        "the document did not load the Mode B client"
    );

    page.eval(
        &mut browser,
        "window.__beckErrors = []; window.__beckRejected = []; \
         document.addEventListener('beck:error', (e) => window.__beckErrors.push(e.detail)); \
         document.addEventListener('beck:rejected', (e) => window.__beckRejected.push(e.detail));",
    )
    .await;

    // Mode B is live when the kernel holds the bundle and interactions are being captured — which
    // is the end of an *asynchronous* load, and is not the same moment as "the scripts ran". A
    // test that interacts before this sees a click reach nothing, and the page never move; that is
    // how this suite failed the first time it was written (`docs/94` §94.7).
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;
    let errors = page.eval(&mut browser, "window.__beckErrors").await;
    assert_eq!(
        errors.as_array().map(|a| a.len()),
        Some(0),
        "the client reported {errors}"
    );

    // Add a card. In Mode B this is applied *locally* first: the assertion below is that the DOM
    // has it, and the one after is that the server ends up agreeing.
    page.eval(
        &mut browser,
        "(() => { const i = document.querySelector('input[data-b-enter]'); \
          i.value = 'written in the browser'; \
          i.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true})); })()",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('written in the browser')",
    )
    .await;

    // The server has it too, once its own fold has run — same command, same `validate`.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if serving
            .rendered("ana")
            .await
            .contains("written in the browser")
        {
            break;
        }
        assert!(
            deadline > std::time::Instant::now(),
            "the server never saw it"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // Move it, by clicking the card the way the page says to.
    page.eval(
        &mut browser,
        "document.querySelector('[data-b-click*=\"Move\"]').click()",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.querySelectorAll('section')[1].innerHTML.includes('written in the browser')",
    )
    .await;

    assert_eq!(
        page.eval(&mut browser, "window.__beckRejected.length")
            .await,
        0,
        "the server refused a command the client accepted"
    );

    // The claim the whole mode rests on, in a real DOM: what the browser rendered locally is what
    // the server would have sent (`docs/94` §94.5).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (there, here) = (
            dom(&page, &mut browser).await,
            serving.rendered("ana").await,
        );
        if there == here {
            break;
        }
        assert!(
            deadline > std::time::Instant::now(),
            "the browser rendered a different page than the server would have\n\
             browser: {there}\n server: {here}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

// A Mode B client refuses what the program would refuse, locally and without a round trip. That is
// gated in `mode_b.rs` rather than here, and the reason is a property of the example rather than a
// gap: no interaction `board.beck`'s page offers *can* be refused — the ids are freshly minted, the
// moves are computed to be legal, and the residue's own handler drops an empty input before any
// command exists. A browser test would have to reach past the page to manufacture one, and a test
// that has to bypass the page is not testing the browser.

/// Reloading is a fresh subscription, and the page comes back.
///
/// A Mode B client resumes from nothing — it holds the state, and after a reload it holds `init`
/// again (`docs/94` §94.5). This is the assertion that the `seq` a reloaded tab claims is the one
/// that gets it a state rather than a gap it cannot apply.
#[tokio::test]
async fn mode_b_survives_a_reload() {
    let Some(binary) = browser::available() else {
        return;
    };
    if !point_at_the_kernel() {
        eprintln!("skipped: no kernel to serve.");
        return;
    }

    let serving = Serving::start(example("board.beck")).await;
    // A card in the log before the browser ever connects, so the reload has something to restore.
    let command = serving
        .app
        .runtime()
        .decode_command(&serde_json::json!({"c":"Add","id":"c1","text":"from before"}))
        .expect("decodes");
    serving
        .app
        .propose("k1".into(), "ana".into(), command)
        .await
        .expect("accepted");

    let mut browser = Browser::launch(&binary).await;
    let mut page = browser.open(&serving.url()).await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('from before')",
    )
    .await;

    let url = serving.url();
    page.navigate(&mut browser, &url).await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('from before')",
    )
    .await;
    assert_eq!(
        dom(&page, &mut browser).await,
        serving.rendered("ana").await,
        "the reloaded page is not the page the server would have sent"
    );
}

/// Keeps `Value` referenced: the harness builds commands through the runtime's own decoder.
#[allow(dead_code)]
fn _value(_: Value) {}
