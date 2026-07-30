// End-to-end, in a real browser: the running example driven by Playwright (§4.8's last row).
//
// What this proves that a Rust test cannot: the patch protocol is applied correctly by the actual
// thin client against an actual DOM, the SSR output and the patch stream agree about tree shape,
// and a subscriber survives the death of the server it was talking to.
//
//   NODE_PATH=$(npm root -g) node phase0/tests/browser.mjs
//
// Requires a build first: cargo build --release -p beck-p0-server

import { createRequire } from "node:module";
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "node:net";
import assert from "node:assert/strict";

// `require` rather than `import`, so a globally installed Playwright on NODE_PATH is found —
// this suite deliberately has no package.json and no node_modules of its own.
const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch {
  console.error("playwright not found. Try: NODE_PATH=$(npm root -g) node phase0/tests/browser.mjs");
  process.exit(2);
}

const here = dirname(fileURLToPath(import.meta.url));
const BINARY = join(here, "..", "target", "release", "beck-p0");
const workdir = mkdtempSync(join(tmpdir(), "beck-p0-browser-"));
const logPath = join(workdir, "app.redb");

const freePort = () =>
  new Promise((resolve) => {
    const probe = createServer();
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function startServer(port) {
  const child = spawn(
    BINARY,
    ["run", "--store", "redb", "--redb-path", logPath, "--addr", `127.0.0.1:${port}`],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
  child.stderr.on("data", (chunk) => {
    const line = chunk.toString();
    if (line.includes("ERROR")) process.stderr.write(line);
  });
  for (let i = 0; i < 100; i++) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/healthz`);
      if (response.ok) return child;
    } catch {}
    await sleep(50);
  }
  throw new Error("server never became healthy");
}

const steps = [];
const step = async (name, fn) => {
  await fn();
  steps.push(name);
  console.log(`  ok  ${name}`);
};

const port = await freePort();
let server = await startServer(port);
const base = `http://127.0.0.1:${port}`;
const browser = await chromium.launch();
let failure = null;

try {
  // --- first paint is the data, not a shell ------------------------------
  await step("SSR ships the view, not a loading state", async () => {
    const html = await (await fetch(`${base}/?actor=alice`)).text();
    assert.match(html, /<h1>todos<\/h1>/);
    assert.match(html, /0 remaining/);
    assert.match(html, /data-b-seq="\d+"/);
    assert.doesNotMatch(html, /loading/i);
  });

  const alice = await browser.newPage();
  await alice.goto(`${base}/?actor=alice`);

  await step("adding a todo round-trips through the log and comes back as a patch", async () => {
    await alice.fill("input", "write the report");
    await alice.press("input", "Enter");
    await alice.waitForSelector("li:has-text('write the report')", { timeout: 5000 });
    assert.equal(await alice.textContent("footer"), "1 remaining");
  });

  await step("clicking a todo toggles it, and only the attribute changes", async () => {
    const before = await alice.evaluate(() => document.querySelector("li span").firstChild);
    await alice.click("li span");
    await alice.waitForSelector("li.done", { timeout: 5000 });
    assert.equal(await alice.textContent("footer"), "0 remaining");
    // The text node was patched in place, not replaced: identity is preserved across a patch,
    // which is what keeps focus and scroll position intact (§5.1).
    assert.equal(before, await alice.evaluate(() => document.querySelector("li span").firstChild));
  });

  await step("a second browser sees the first one's todos", async () => {
    const bob = await browser.newPage();
    await bob.goto(`${base}/?actor=bob`);
    await bob.waitForSelector("li:has-text('write the report')", { timeout: 5000 });

    await bob.fill("input", "review the numbers");
    await bob.press("input", "Enter");
    await alice.waitForSelector("li:has-text('review the numbers')", { timeout: 5000 });
    assert.equal((await alice.$$("li")).length, 2);
    await bob.close();
  });

  await step("the client is enemy territory: bob cannot toggle alice's todo", async () => {
    const bob = await browser.newPage();
    const rejections = [];
    await bob.goto(`${base}/?actor=bob`);
    await bob.waitForSelector("li", { timeout: 5000 });
    await bob.evaluate(() =>
      document
        .getElementById("b-root")
        .addEventListener("beck:rejected", (e) => (window.__rejected = e.detail.e)),
    );
    // Alice's row, clicked by bob.
    await bob.click("li:has-text('write the report') span");
    await bob.waitForFunction(() => window.__rejected !== undefined, { timeout: 5000 });
    rejections.push(await bob.evaluate(() => window.__rejected));
    assert.deepEqual(rejections, ["not the owner"]);
    await bob.close();
  });

  await step("deleting removes exactly one row", async () => {
    await alice.click("li:has-text('write the report') button");
    await alice.waitForFunction(() => document.querySelectorAll("li").length === 1, {
      timeout: 5000,
    });
    assert.equal(await alice.textContent("li span"), "review the numbers");
  });

  // --- the process dies under the browser --------------------------------
  await step("a subscriber survives the death of its server", async () => {
    server.kill("SIGKILL");
    await sleep(200);
    server = await startServer(port);

    // The tab was never reloaded: the thin client reconnects on its own backoff and resumes the
    // subscription by (subscription, seq) against a process that folded the log from scratch.
    await alice.fill("input", "after the crash");
    await alice.press("input", "Enter");
    await alice.waitForSelector("li:has-text('after the crash')", { timeout: 15000 });
    assert.equal((await alice.$$("li")).length, 2);
  });

  await step("a reload after the crash is served from the recovered fold", async () => {
    const reloaded = await browser.newPage();
    await reloaded.goto(`${base}/?actor=alice`);
    // No socket needed: this is the SSR first paint, rendered from state that exists only because
    // the log was replayed.
    assert.equal((await reloaded.$$("li")).length, 2);
    await reloaded.waitForSelector("li:has-text('review the numbers')");
    await reloaded.close();
  });

  await step("the log outlives every process that touched it", async () => {
    // redb takes an exclusive lock on the file, so the CLI can only read the log once the server
    // has let go of it. That is a real constraint of the rung-0 substrate, not an accident of this
    // test — see the Phase 0 report.
    server.kill("SIGKILL");
    await sleep(200);
    const replay = execFileSync(BINARY, [
      "replay",
      "--store",
      "redb",
      "--redb-path",
      logPath,
      "--genesis",
    ]).toString();
    assert.match(replay, /replayed to\s+\d+/);
    const verify = execFileSync(BINARY, [
      "verify",
      "--store",
      "redb",
      "--redb-path",
      logPath,
    ]).toString();
    assert.match(verify, /replay is exact/);
  });
} catch (e) {
  failure = e;
} finally {
  await browser.close();
  server.kill("SIGKILL");
  rmSync(workdir, { recursive: true, force: true });
}

if (failure) {
  console.error(`\nFAILED after ${steps.length} steps:\n`, failure);
  process.exit(1);
}
console.log(`\n${steps.length} browser steps passed`);
