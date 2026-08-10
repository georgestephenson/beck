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
//!
//! The playground is here too, for the same reason and then one more: rung A and rung B are a
//! page, a worker and two iframes passing ports to each other, and *none* of that exists in any
//! other suite. `docs/96` is the report; `playground.rs` gates what the module answers, and these
//! gate that a browser can get the answers out of it.

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

/// Build the kernel this suite is about, and point the server at it. Once.
///
/// **Built, not found.** Reading whatever `.wasm` happens to be on disk is how this suite spent an
/// afternoon testing a kernel three commits old: the shim asked it for an operation it did not
/// have, the reply was an error nobody looked at, and the page quietly did nothing. A browser
/// suite that can be stale about the thing it is testing is worse than no browser suite.
///
/// `BECK_KERNEL` is the operator's interface (`beck_rt::http::kernel_path`) and it is process-wide,
/// so it is set here rather than per test: every test in this binary wants the same value, and the
/// alternative — a runtime API that exists only for a test — would be a worse seam than an
/// environment variable that already exists for a real reason.
fn point_at_the_kernel() -> bool {
    static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let built = std::process::Command::new(env!("CARGO"))
            .current_dir(root())
            .args([
                "build",
                "-p",
                "beck-wasm",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .output();
        let module = root().join("target/wasm32-unknown-unknown/release/beck_wasm.wasm");
        if !built.is_ok_and(|o| o.status.success()) || !module.is_file() {
            return false;
        }
        std::env::set_var("BECK_KERNEL", &module);
        true
    })
}

/// A running application, on a port of its own — and able to go away and come back.
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
        let (shutdown, _) = tokio::sync::watch::channel(false);
        let mut serving = Serving {
            app,
            addr,
            shutdown,
        };
        serving.listen().await;
        serving
    }

    /// Start accepting. The application outlives the server, which is what makes stopping and
    /// starting a *reconnect* rather than a different program.
    async fn listen(&mut self) {
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        self.shutdown = shutdown;
        let app = self.app.clone();
        let addr = self.addr;
        tokio::spawn(async move {
            let _ = beck_rt::http::serve(app, addr, rx).await;
        });
        // The listener binds inside `serve`, so give it the moment that takes before a browser is
        // told the server is back.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    /// Stop accepting, and drop every connection.
    ///
    /// This is how a browser is taken offline here rather than Chromium's network emulation, which
    /// does not close an already-open websocket: a socket that stays up is not offline, and a test
    /// that believes it is proves nothing. A server that goes away is also the more realistic
    /// event — it is what a deploy looks like.
    async fn stop(&mut self) {
        let _ = self.shutdown.send(true);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
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

browser_test! {
/// The thin client connects, renders and applies a patch — in a browser, for the first time.
///
/// The `contains` assertions are about *the DOM after a round trip*: the card is not in the
/// server-rendered document, so its appearance is a patch this browser received over a websocket
/// and applied with `beck-patch.js`.
async fn mode_a_applies_the_servers_patches() {
    let Some(mut browser) = browser::shared().await else {
        return;
    };
    let serving = Serving::start(example("todo.beck")).await;
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
}

// --------------------------------------------------------------- Mode B

browser_test! {
/// The whole of Mode B, in the program it was written for.
///
/// Loads the kernel and the bundle over HTTP, renders locally from a data patch, applies a command
/// optimistically, and ends up with the page the server would have rendered.
async fn mode_b_renders_in_the_browser_and_guesses_ahead_of_the_server() {
    let Some(mut browser) = browser::shared().await else {
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
}

// A Mode B client refuses what the program would refuse, locally and without a round trip. That is
// gated in `mode_b.rs` rather than here, and the reason is a property of the example rather than a
// gap: no interaction `board.beck`'s page offers *can* be refused — the ids are freshly minted, the
// moves are computed to be legal, and the residue's own handler drops an empty input before any
// command exists. A browser test would have to reach past the page to manufacture one, and a test
// that has to bypass the page is not testing the browser.

browser_test! {
/// Reloading is a fresh subscription, and the page comes back.
///
/// A Mode B client resumes from nothing — it holds the state, and after a reload it holds `init`
/// again (`docs/94` §94.5). This is the assertion that the `seq` a reloaded tab claims is the one
/// that gets it a state rather than a gap it cannot apply.
async fn mode_b_survives_a_reload() {
    let Some(mut browser) = browser::shared().await else {
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
        .propose("k1".into(), "ana", command)
        .await
        .expect("accepted");

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
}

/// Keeps `Value` referenced: the harness builds commands through the runtime's own decoder.
#[allow(dead_code)]
fn _value(_: Value) {}

// --------------------------------------------------------------- offline (D7 rung 2)

browser_test! {
/// The claim [`docs/10-decisions.md`](../../../../docs/10-decisions.md) D7 makes for Mode B, with
/// the server gone.
///
/// > Offline-tolerant v1: a Mode B component holds a local copy of its state and queues commands
/// > while offline, replaying them on reconnect.
///
/// Three things in one test, because they are one property: with the server gone the page still
/// takes an interaction; reconnecting sends what was queued; and the log gets **exactly one** of
/// each command, because the idempotency key the queue kept is the one `App::propose` already
/// de-duplicates by. Offline tolerance is the fold plus that key — no new agreement between the
/// two sides, which is what D7 predicted and is worth checking rather than assuming.
async fn mode_b_works_with_the_server_gone_and_catches_up_when_it_returns() {
    let Some(mut browser) = browser::shared().await else {
        return;
    };
    if !point_at_the_kernel() {
        eprintln!("skipped: no kernel to serve.");
        return;
    }

    let mut serving = Serving::start(example("board.beck")).await;
    let page = browser.open(&serving.url()).await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;

    // A card while connected, so the local copy holds something the server holds too.
    add_card(&page, &mut browser, "before the tunnel").await;
    wait_for_server(&serving, "before the tunnel").await;
    let connected = serving.app.head();

    // Into the tunnel.
    serving.stop().await;

    // The interaction still lands: the fold is here, so the page moves with no server at all.
    add_card(&page, &mut browser, "written in the tunnel").await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('written in the tunnel')",
    )
    .await;
    assert_eq!(
        serving.app.head(),
        connected,
        "a command reached the log with the server stopped"
    );

    // What is *not* tested here, because it does not work: reloading while the server is gone. The
    // document comes from the server, so a reload with nothing listening gets a browser error page
    // and the local copy is never consulted — the shell would have to be cached by a service
    // worker, and there is none (`docs/94` §94.13).

    // Out of the tunnel. The queue goes up as soon as a socket opens.
    serving.listen().await;
    wait_for_server(&serving, "written in the tunnel").await;

    // Exactly once, however many times the client retried: the command carried the same id each
    // time, and the server de-duplicates by it.
    assert_eq!(
        serving.app.head(),
        connected + 1,
        "the queued command was appended more than once"
    );

    // And the two sides agree again.
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
            "the reconnected browser and the server disagree\nbrowser: {there}\n server: {here}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
}

browser_test! {
/// A snapshot of another program is refused rather than folded into this one.
///
/// A deployment that changes the command channel's types changes the wire id (§4.3), and a tab that
/// comes back to it is holding a copy of something else. Restoring it would be a state no log ever
/// produced.
async fn a_local_copy_of_another_program_is_dropped() {
    let Some(mut browser) = browser::shared().await else {
        return;
    };
    if !point_at_the_kernel() {
        eprintln!("skipped: no kernel to serve.");
        return;
    }

    let serving = Serving::start(example("board.beck")).await;
    let mut page = browser.open(&serving.url()).await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;
    add_card(&page, &mut browser, "mine").await;
    wait_for_server(&serving, "mine").await;
    // The copy is written on a trailing timer, so wait for it rather than for a duration.
    page.wait_for(
        &mut browser,
        "Object.keys(localStorage).some(k => k.startsWith('beck:'))",
    )
    .await;

    // Forge a copy of a different program under this one's key.
    page.eval(
        &mut browser,
        "(() => { const k = Object.keys(localStorage).find(k => k.startsWith('beck:')); \
          const v = JSON.parse(localStorage.getItem(k)); v.wire = '0000000000000000'; \
          localStorage.setItem(k, JSON.stringify(v)); return k; })()",
    )
    .await;

    let url = serving.url();
    page.navigate(&mut browser, &url).await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;
    // The page is still right — it came from the server rather than from the forged copy.
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('mine')",
    )
    .await;
    assert_eq!(
        dom(&page, &mut browser).await,
        serving.rendered("ana").await
    );
}
}

async fn add_card(page: &Page, browser: &mut Browser, text: &str) {
    page.eval(
        browser,
        &format!(
            "(() => {{ const i = document.querySelector('input[data-b-enter]'); \
              i.value = {text:?}; \
              i.dispatchEvent(new KeyboardEvent('keydown', {{key: 'Enter', bubbles: true}})); }})()"
        ),
    )
    .await;
}

async fn wait_for_server(serving: &Serving, text: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if serving.rendered("ana").await.contains(text) {
            return;
        }
        assert!(
            deadline > std::time::Instant::now(),
            "the server never saw `{text}`"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

browser_test! {
/// A cold start with the server gone: the tab is closed, reopened, and the application is there.
///
/// This is the half of D7 rung 2 the queue alone cannot reach. The state was already in the
/// browser; what was not was the *document*, the scripts and the kernel, all of which come from the
/// server — so a reload with nothing listening never got as far as consulting the local copy. The
/// service worker caches that shell, network-first, keyed by the program's wire id.
async fn mode_b_cold_starts_with_the_server_gone() {
    let Some(mut browser) = browser::shared().await else {
        return;
    };
    if !point_at_the_kernel() {
        eprintln!("skipped: no kernel to serve.");
        return;
    }

    let mut serving = Serving::start(example("board.beck")).await;
    let mut page = browser.open(&serving.url()).await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;
    add_card(&page, &mut browser, "cached before the outage").await;
    wait_for_server(&serving, "cached before the outage").await;

    // The worker has to have taken control before it can answer a navigation.
    page.wait_for(&mut browser, "!!navigator.serviceWorker.controller")
        .await;
    // And the local copy has to have been written, since that is what the reloaded page renders.
    page.wait_for(
        &mut browser,
        "Object.keys(localStorage).some(k => k.startsWith('beck:'))",
    )
    .await;

    let connected = serving.app.head();
    serving.stop().await;

    // A reload with nothing listening. Everything comes out of the browser: the document from the
    // worker's cache, the state from `localStorage`.
    let url = serving.url();
    page.navigate(&mut browser, &url).await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').dataset.bReady === 'b'",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('cached before the outage')",
    )
    .await;

    // And it is an application, not a photograph: the fold is here, so it still takes a command.
    add_card(&page, &mut browser, "added while it was down").await;
    page.wait_for(
        &mut browser,
        "document.getElementById('b-root').innerHTML.includes('added while it was down')",
    )
    .await;
    assert_eq!(
        serving.app.head(),
        connected,
        "a command reached the log with the server stopped"
    );

    // The server returns, and the queue that survived a page load goes up.
    serving.listen().await;
    wait_for_server(&serving, "added while it was down").await;
    assert_eq!(
        serving.app.head(),
        connected + 1,
        "the queued command was appended more than once"
    );
}
}

// --------------------------------------------------------------- the playground (docs/17, docs/96)

/// Build the playground module and point the server at it. Once, for the same reason
/// [`point_at_the_kernel`] is once — a browser suite that can be stale about the thing it is
/// testing is worse than no browser suite.
fn point_at_the_playground() -> bool {
    static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let built = std::process::Command::new(env!("CARGO"))
            .current_dir(root())
            .args([
                "build",
                "-p",
                "beck-play",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .output();
        let module = root().join("target/wasm32-unknown-unknown/release/beck_play.wasm");
        if !built.is_ok_and(|o| o.status.success()) || !module.is_file() {
            return false;
        }
        std::env::set_var("BECK_PLAYGROUND", &module);
        true
    })
}

/// The playground, served on a port of its own — which is what makes each test's page a different
/// origin, and therefore isolated for free.
struct Playing {
    addr: SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl Playing {
    async fn start() -> Option<Playing> {
        if !point_at_the_playground() {
            eprintln!(
                "skipped: no playground module to serve. Build it with \
                 `cargo build -p beck-play --release --target wasm32-unknown-unknown`."
            );
            assert!(
                std::env::var("BECK_REQUIRE_WASM").as_deref() != Ok("1"),
                "BECK_REQUIRE_WASM=1 but there is no playground module to serve"
            );
            return None;
        }
        let addr = free_port();
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let _ = beck_play::serve::serve(addr, rx).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Some(Playing { addr, shutdown })
    }

    fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }
}

impl Drop for Playing {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

browser_test! {
/// Rung A: the compiler, in the tab, with no server deciding anything.
///
/// The page is static files and a WebAssembly module. What this asserts is that a *browser* gets
/// the compiler's answers out of it — the placement table for the program in the editor, and a
/// real diagnostic, with its code and its span, for one that does not compile.
async fn the_playground_compiles_in_the_browser() {
    let Some(mut browser) = browser::shared().await else {
        return;
    };
    let Some(playing) = Playing::start().await else {
        return;
    };
    let page = browser.open(&playing.url()).await;

    // The page says when it is live, for the same reason the Mode B client does: everything here
    // is behind an asynchronous load, and a test that types before it is ready types into nothing.
    page.wait_for(&mut browser, "document.body.dataset.ready === '1'")
        .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('status').textContent === 'compiles'",
    )
    .await;

    // The placement of the program in the editor, derived in the browser.
    page.eval(
        &mut browser,
        "[...document.querySelectorAll('#tabs button')].find(b => b.textContent === 'Placement').click()",
    )
    .await;
    let placement = page.text(&mut browser, "document.getElementById('out').textContent").await;
    assert!(
        placement.contains("page") && placement.contains("client"),
        "the browser did not derive a placement:\n{placement}"
    );

    // And a program that does not compile gets the compiler's own diagnostic, not a message the
    // page wrote.
    page.eval(
        &mut browser,
        "(() => { const s = document.getElementById('source'); \
          s.value = 'def broken(x: Int) -> Int:\\n    return x + \"nope\"\\n'; \
          s.dispatchEvent(new Event('input')); })()",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('status').classList.contains('bad')",
    )
    .await;
    let diagnostics = page.text(&mut browser, "document.getElementById('out').textContent").await;
    assert!(
        diagnostics.contains("error[B0") && diagnostics.contains("playground.beck"),
        "the browser did not show a compiler diagnostic:\n{diagnostics}"
    );
}
}

browser_test! {
/// Rung B: the whole application in one tab, and two clients of it.
///
/// docs/17 §17.2's two demos, in a browser: **multiplayer in one tab** — a click in ana's iframe
/// reaches bo's page through the worker's log and the same patch protocol a deployed application
/// speaks — and **time travel**, where dragging the scrubber folds the log again from genesis.
///
/// Nothing in this test talks to a server: the module was fetched once, and every answer after
/// that came out of the worker.
async fn the_playground_runs_the_application_and_two_clients_of_it() {
    let Some(mut browser) = browser::shared().await else {
        return;
    };
    let Some(playing) = Playing::start().await else {
        return;
    };
    let page = browser.open(&playing.url()).await;
    page.wait_for(&mut browser, "document.body.dataset.ready === '1'")
        .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('run').disabled === false",
    )
    .await;
    page.eval(&mut browser, "document.getElementById('run').click()")
        .await;

    // Both clients are live when the residue says so — `data-b-ready` is set by `beck-thin.js` on
    // its welcome frame, which is the same signal a deployed page gives.
    for frame in ["client-ana", "client-bo"] {
        page.wait_for(
            &mut browser,
            &format!(
                "document.getElementById('{frame}').contentDocument \
                 ?.getElementById('b-root')?.dataset.bReady === 'a'"
            ),
        )
        .await;
    }

    // ana clicks. The command goes up her port, through the one merge point, into the log — and
    // the frame that comes back to *bo* is what makes this a fanout rather than a mirror.
    page.eval(
        &mut browser,
        "document.getElementById('client-ana').contentDocument \
         .querySelector('button.up').click()",
    )
    .await;
    for frame in ["client-ana", "client-bo"] {
        page.wait_for(
            &mut browser,
            &format!(
                "document.getElementById('{frame}').contentDocument \
                 .querySelector('.count').textContent === '1'"
            ),
        )
        .await;
    }

    // bo clicks too, and ana sees it. Two subscriptions, one log, one order.
    page.eval(
        &mut browser,
        "document.getElementById('client-bo').contentDocument \
         .querySelector('button.up').click()",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.getElementById('client-ana').contentDocument \
         .querySelector('.count').textContent === '2'",
    )
    .await;

    // The log holds two events, and the page shows them.
    page.wait_for(
        &mut browser,
        "document.querySelectorAll('#log tbody tr').length === 2",
    )
    .await;

    // Time travel: fold the same log to position 1 and to position 0. Both are computed by the
    // program's own fold, in the tab, from genesis.
    page.eval(
        &mut browser,
        "(() => { const s = document.getElementById('scrub'); s.value = '1'; \
          s.dispatchEvent(new Event('input')); })()",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.querySelector('#preview .count')?.textContent === '1'",
    )
    .await;
    page.eval(
        &mut browser,
        "(() => { const s = document.getElementById('scrub'); s.value = '0'; \
          s.dispatchEvent(new Event('input')); })()",
    )
    .await;
    page.wait_for(
        &mut browser,
        "document.querySelector('#preview .count')?.textContent === '0'",
    )
    .await;

    // And the live clients did not move while history was being dragged: a scrubber that moved the
    // application would be an undo, not a replay.
    assert_eq!(
        page.text(
            &mut browser,
            "document.getElementById('client-ana').contentDocument \
             .querySelector('.count').textContent"
        )
        .await,
        "2"
    );
}
}
