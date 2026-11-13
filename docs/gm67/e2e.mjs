// GM-67 end-to-end driver (keyboard and feedback checks added by GM-69): launches the real GitMiro exe (WebView2)
// and drives it over the DevTools protocol.
// usage: node e2e.mjs <exe> <fixturesDir>
import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync, chmodSync, existsSync } from "node:fs";
import { join } from "node:path";

const [exe, fixtures] = process.argv.slice(2);
const work = join(fixtures, "_work");
rmSync(work, { recursive: true, force: true });
mkdirSync(work, { recursive: true });
writeFileSync(join(work, "empty-gitconfig"), "");

const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
// noid: a repository with no author identity and a hook that can be told to fail.
const noid = join(fixtures, "noid");
rmSync(noid, { recursive: true, force: true });
mkdirSync(noid);
git(noid, "init", "-q", "-b", "main");
git(noid, "config", "commit.gpgsign", "false");
git(noid, "config", "core.autocrlf", "false");
writeFileSync(join(noid, "a.txt"), "a\n");
git(noid, "add", ".");
execFileSync("git", ["-C", noid, "-c", "user.name=Seed", "-c", "user.email=seed@example.invalid", "commit", "-q", "-m", "seed"]);
writeFileSync(join(noid, "b.txt"), "b\n");
writeFileSync(join(noid, ".git", "hooks", "pre-commit"), '#!/bin/sh\nif [ -f .git/fail ]; then echo "hook says no" >&2; exit 1; fi\nexit 0\n');
// plain: slow hook to hold a commit open.
const plain = join(fixtures, "plain");
writeFileSync(join(plain, ".git", "hooks", "pre-commit"), "#!/bin/sh\nsleep 3\nexit 0\n");

const PORT = 9223;
const env = {
  ...process.env,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
  WEBVIEW2_USER_DATA_FOLDER: join(work, "webview-data"),
  GIT_CONFIG_GLOBAL: join(work, "empty-gitconfig"),
  GIT_CONFIG_NOSYSTEM: "1",
};
const app = spawn(exe, [], { env, stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? "  -> " + detail : ""}`);
};

async function targetUrl() {
  for (let i = 0; i < 100; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      const page = list.find((t) => t.type === "page");
      if (page) return page.webSocketDebuggerUrl;
    } catch {}
    await sleep(200);
  }
  throw new Error("WebView2 DevTools endpoint not reachable");
}

const ws = new WebSocket(await targetUrl());
await new Promise((r) => (ws.onopen = r));
let nextId = 1;
const pending = new Map();
const events = [];
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  } else if (msg.method) events.push(msg);
};
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = nextId++;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
const ev = async (expression) => {
  const r = await send("Runtime.evaluate", { expression: `(async()=>{${expression}})()`, awaitPromise: true, returnByValue: true });
  if (r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails.exception?.description ?? r.result.exceptionDetails));
  return r.result.result.value;
};
const waitFor = async (expression, timeout = 15000) => {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try {
      if (await ev(`return !!(${expression})`)) return true;
    } catch {}
    await sleep(100);
  }
  return false;
};
// Real key presses (GM-69). Enter needs its text to activate a button.
const KEYS = { Enter: [13, "\r"], Escape: [27], Tab: [9], F6: [117] };
const press = async (name, modifiers = 0) => {
  const [vk, text] = KEYS[name];
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: name, code: name, windowsVirtualKeyCode: vk, modifiers, text });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: name, code: name, windowsVirtualKeyCode: vk, modifiers });
};
const focusLabel = (label) => ev(`[...document.querySelectorAll('button')].find(b=>b.getAttribute('aria-label')===${JSON.stringify(label)}).focus()`);
const activeLabel = () => ev("const a=document.activeElement; return a ? (a.getAttribute('aria-label')||a.id||a.tagName) : null");
const statusLine = () => ev("return document.querySelector('.titlebar [role=status]')?.textContent ?? null");

await send("Runtime.enable");
await send("Log.enable");
await send("Page.enable");
await send("Page.addScriptToEvaluateOnNewDocument", {
  source: "window.__csp=[];document.addEventListener('securitypolicyviolation',e=>window.__csp.push(e.violatedDirective+' '+e.blockedURI));window.addEventListener('unhandledrejection',e=>(window.__errors=window.__errors||[]).push('rejection '+String(e.reason)));window.addEventListener('error',e=>(window.__errors=window.__errors||[]).push(String(e.message)));",
});

try {
  // Seed recents and reload so the welcome screen lists the fixtures.
  await waitFor("document.body");
  const paths = ["big", "merge", "plain", "noid"].map((n) => join(fixtures, n));
  await ev(`localStorage.setItem('gitmiro.recentRepositories', ${JSON.stringify(JSON.stringify(paths))}); location.reload();`);
  await sleep(500);
  check("welcome screen lists the recent fixtures", await waitFor("document.querySelectorAll('.recent-repository').length===4"));

  const openRecent = async (name) => {
    await ev(`[...document.querySelectorAll('.recent-repository')].find(b=>b.textContent.startsWith(${JSON.stringify(name)})).click()`);
    return waitFor(`document.querySelector('.repository-heading h1')?.textContent===${JSON.stringify(name)}`);
  };
  const backToWelcome = async () => {
    // No UI route back to the welcome screen: reload keeps only the recents.
    await ev("location.reload()");
    await sleep(500);
    await waitFor("document.querySelectorAll('.recent-repository').length>0");
  };

  // --- big repository: focus on open, infinite scroll
  check("open big repository", await openRecent("big"));
  check("focus moves to the repository title when it is opened", await ev("return document.activeElement===document.querySelector('.repository-heading h1')"));
  check("first page has 500 commits", await waitFor("document.querySelectorAll('.commit-list li').length===500"));
  const opened = await statusLine();
  check("status line (role=status) reports the opened repository", opened === "Opened big on main", opened);
  await ev("const c=document.querySelector('.commit-history'); c.scrollTop=c.scrollHeight;");
  check("scrolling the history container loads the 2nd page", await waitFor("document.querySelectorAll('.commit-list li').length===1000"));
  await ev("const c=document.querySelector('.commit-history'); c.scrollTop=c.scrollHeight;");
  check("scrolling again loads the last page (1300) and hides the button", await waitFor("document.querySelectorAll('.commit-list li').length===1300 && !document.querySelector('.load-more')"));

  // --- GM-69: pane navigation, keyboard diff, disabled look
  await ev("document.activeElement.blur()");
  const panes = [];
  for (const shift of [0, 0, 0, 8]) {
    await press("F6", shift);
    panes.push(await ev("return ['reference-sidebar','history-panel','status-column'].find(c=>document.activeElement.closest('.'+c))||'none'"));
  }
  check("F6 cycles references, history, status; Shift+F6 goes back", panes.join(",") === "reference-sidebar,history-panel,status-column,history-panel", panes.join(","));
  await focusLabel("View diff for f.txt");
  await press("Enter");
  check("Enter on Diff opens the diff view", await waitFor("document.querySelector('.file-diff-patch')"));
  await ev("document.querySelector('.file-diff-view button').focus()");
  await press("Escape");
  check("Escape closes the diff view", await waitFor("!document.querySelector('.file-diff-view')", 3000));
  check("focus returns to the Diff button that opened it", (await activeLabel()) === "View diff for f.txt", await activeLabel());
  const commitCursor = await ev("const b=document.querySelector('.commit-composer .button-primary'); return b.disabled+':'+getComputedStyle(b).cursor");
  check("a disabled (not busy) button has no progress cursor", commitCursor === "true:default", commitCursor);

  // --- focus is not stolen by a status refresh (stage replaces the repository object)
  await ev("document.getElementById('commit-message').focus()");
  await ev("[...document.querySelectorAll('.status-action')].find(b=>b.getAttribute('aria-label')==='Stage f.txt').click()");
  await waitFor("[...document.querySelectorAll('.status-action')].some(b=>b.getAttribute('aria-label')==='Unstage f.txt')");
  await sleep(300);
  check("a stage (state refresh) keeps the focus on the commit message", await ev("return document.activeElement?.id==='commit-message'"));
  check("status line reports the stage", (await statusLine()) === "Staged f.txt", await statusLine());
  // GM-69: unstaging with the keyboard removes the focused button; the focus stays in the staged list.
  await focusLabel("Unstage f.txt");
  await press("Enter");
  await waitFor("[...document.querySelectorAll('.status-action')].some(b=>b.getAttribute('aria-label')==='Stage f.txt')");
  await sleep(300);
  check("after a keyboard unstage the focus moves to the next row of the staged list", (await activeLabel()) === "Unstage new.txt", await activeLabel());

  // --- merge with an empty index
  await backToWelcome();
  check("open merge repository", await openRecent("merge"));
  check("amend is disabled while a merge is in progress", await ev("return document.querySelector('.amend-option input')?.disabled===true"));
  const commitLabel = await ev("return [...document.querySelectorAll('.button-primary')].map(b=>b.textContent+':'+b.disabled).join('|')");
  check("merge with nothing staged offers 'Commit merge' prefilled from MERGE_MSG and enabled", commitLabel.includes("Commit merge:false"), commitLabel);
  await ev(`const t=document.getElementById('commit-message'); const set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set; set.call(t,'Merge side'); t.dispatchEvent(new Event('input',{bubbles:true}));`);
  check("commit merge button becomes enabled with an empty index", await waitFor("[...document.querySelectorAll('.button-primary')].some(b=>b.textContent==='Commit merge' && !b.disabled)"));
  await ev("[...document.querySelectorAll('.button-primary')].find(b=>b.textContent==='Commit merge').click()");
  await waitFor("document.querySelector('.commit-composer .button-primary')?.textContent==='Commit staged changes'", 15000);
  const parents = git(join(fixtures, "merge"), "rev-list", "--parents", "-n", "1", "HEAD").trim().split(" ").length;
  check("the merge commit really has two parents", parents === 3, `fields=${parents}`);

  // --- plain: slow hook, concurrent actions must be blocked while committing
  await backToWelcome();
  check("open plain repository", await openRecent("plain"));
  await ev("[...document.querySelectorAll('.status-action')].find(b=>b.getAttribute('aria-label')==='Stage b.txt').click()");
  await waitFor("[...document.querySelectorAll('.status-action')].some(b=>b.getAttribute('aria-label')==='Unstage b.txt')");
  await ev("[...document.querySelectorAll('.status-action')].find(b=>b.getAttribute('aria-label')==='View diff for b.txt').click()");
  check("diff of the staged file is open before the commit", await waitFor("document.querySelector('.file-diff-view')"));
  await ev("[...document.querySelectorAll('.file-diff-view button')].find(b=>b.textContent==='Back to history')&&0");
  await ev(`const t=document.getElementById('commit-message'); const set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set; set.call(t,'slow commit'); t.dispatchEvent(new Event('input',{bubbles:true}));`);
  await waitFor("[...document.querySelectorAll('.button-primary')].some(b=>!b.disabled)");
  await ev("document.getElementById('commit-message').focus()");
  await ev("[...document.querySelectorAll('.button-primary')].find(b=>b.textContent==='Commit staged changes').click()");
  await waitFor("document.querySelector('.commit-composer .button-primary')?.textContent==='Committing…'");
  const during = await ev("return {open:[...document.querySelectorAll('.repository-toolbar button')].every(b=>b.disabled), actions:[...document.querySelectorAll('.status-action')].filter(b=>/^(Unstage|Stage)/.test(b.getAttribute('aria-label')||'')).every(b=>b.disabled)}");
  check("while committing: Open another is disabled", during.open === true);
  check("while committing: stage/unstage buttons are disabled", during.actions === true);
  check("while committing: the status line says so", (await statusLine()) === "Committing…", await statusLine());
  check("slow commit finishes", await waitFor("!document.querySelector('.commit-composer .button-primary')?.textContent.includes('Committing')", 20000));
  check("status line reports the new commit", /^Committed [0-9a-f]{7} on main$/.test(await statusLine()), await statusLine());
  check("the open diff is closed once its staged file is committed (no stale diff)", await waitFor("!document.querySelector('.file-diff-view')", 5000));
  await sleep(300); const fa = await ev("const a=document.activeElement; return a ? a.tagName+'#'+a.id+'.'+a.className : null"); check("focus returns to the commit message after a commit (no drop to <body>)", fa === 'TEXTAREA#commit-message.', fa);
  check("commit landed in the plain repository", git(plain, "log", "-1", "--format=%s").trim() === "slow commit");

  // --- noid: identity dialog behaviour
  await backToWelcome();
  check("open noid repository", await openRecent("noid"));
  await ev("[...document.querySelectorAll('.status-action')].find(b=>b.getAttribute('aria-label')==='Stage b.txt').click()");
  await waitFor("[...document.querySelectorAll('.status-action')].some(b=>b.getAttribute('aria-label')==='Unstage b.txt')");
  await ev(`const t=document.getElementById('commit-message'); t.focus(); const set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set; set.call(t,'with identity'); t.dispatchEvent(new Event('input',{bubbles:true}));`);
  await waitFor("[...document.querySelectorAll('.button-primary')].some(b=>!b.disabled)");
  await ev("document.querySelector('.commit-composer .button-primary').focus(); document.querySelector('.commit-composer .button-primary').click()");
  check("identity dialog opens when no author is configured", await waitFor("document.querySelector('dialog[open]')"));
  check("dialog takes the focus (name field)", await ev("return document.activeElement?.id==='author-name'"));
  // GM-69: really modal. The page behind cannot take the focus and Tab never leaves the dialog.
  check("dialog is modal and the page behind is inert", await ev("const d=document.querySelector('dialog[open]'); document.getElementById('commit-message').focus(); return d.matches(':modal') && document.activeElement.id!=='commit-message'"));
  await ev("document.getElementById('author-name').focus()");
  const outside = [];
  for (let i = 0; i < 12; i++) {
    await press("Tab", i >= 6 ? 8 : 0);
    const where = await ev("const a=document.activeElement; return !a||a===document.body ? 'body' : document.querySelector('dialog[open]').contains(a) ? 'in' : (a.id||a.className)");
    if (where !== "in" && where !== "body") outside.push(where);
  }
  check("Tab and Shift+Tab stay inside the dialog", outside.length === 0, outside.join(","));
  await press("Escape");
  check("Escape closes the identity dialog", await waitFor("!document.querySelector('dialog[open]')", 3000));
  check("focus returns to the commit button", await ev("return document.activeElement===document.querySelector('.commit-composer .button-primary')"));

  // failing hook: error must be visible inside the dialog
  writeFileSync(join(noid, ".git", "fail"), "");
  await ev("document.querySelector('.commit-composer .button-primary').click()");
  await waitFor("document.querySelector('dialog[open]')");
  const fill = (id, value) => ev(`const i=document.getElementById(${JSON.stringify(id)}); const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set; set.call(i,${JSON.stringify(value)}); i.dispatchEvent(new Event('input',{bubbles:true}));`);
  await fill("author-name", "Dialog Author");
  await fill("author-email", "dialog@example.invalid");
  await ev("document.querySelector('dialog[open] .button-primary').click()");
  check("a commit error is shown inside the identity dialog", await waitFor("document.querySelector('dialog[open] .error-message')?.textContent.includes('hook says no')"));
  rmSync(join(noid, ".git", "fail"));
  await ev("document.querySelector('dialog[open] .button-primary').click()");
  check("retry from the dialog commits with the entered identity", await waitFor("!document.querySelector('dialog[open]')", 15000));
  check("author recorded", git(noid, "log", "-1", "--format=%an <%ae>").trim() === "Dialog Author <dialog@example.invalid>");

  // --- GM-69: welcome screen, failed open and removal of a recent entry with the keyboard
  const notRepo = join(work, "not-a-repository");
  mkdirSync(notRepo, { recursive: true });
  await ev(`localStorage.setItem('gitmiro.recentRepositories', ${JSON.stringify(JSON.stringify([notRepo, join(fixtures, "big")]))}); location.reload();`);
  await sleep(500);
  await waitFor("document.querySelectorAll('.recent-repository').length===2");
  await ev("document.querySelector('.recent-repository').focus()");
  await press("Enter");
  check("a failed open shows an alert", await waitFor("document.querySelector('.welcome-panel [role=alert]')"));
  await sleep(300);
  check("after a failed open the focus returns to the entry that was activated", await ev("return document.activeElement===document.querySelector('.recent-repository')"));
  check("after a failed open the status line is empty (no stale 'Opening')", (await statusLine()) === "", await statusLine());
  await focusLabel(`Remove ${notRepo} from recent repositories`);
  await press("Enter");
  await sleep(300);
  check("removing a recent entry moves the focus to the next entry", await ev("return document.querySelectorAll('.recent-repository').length===1 && document.activeElement===document.querySelector('.recent-repository')"));

  // --- capabilities
  const denied = await ev("try{ await window.__TAURI_INTERNALS__.invoke('plugin:window|set_title',{label:'main',value:'x'}); return 'allowed' }catch(e){ return String(e) }");
  check("capability: window title mutation is denied to the page", denied !== "allowed", denied.slice(0, 80));
  const deniedFs = await ev("try{ await window.__TAURI_INTERNALS__.invoke('plugin:path|resolve_directory',{directory:13}); return 'allowed' }catch(e){ return String(e) }");
  check("capability: path plugin is not exposed to the page", deniedFs !== "allowed", deniedFs.slice(0, 80));
  const listen = await ev("try{ const id=await window.__TAURI_INTERNALS__.invoke('plugin:event|listen',{event:'tauri://focus',target:{kind:'Window',label:'main'},handler:window.__TAURI_INTERNALS__.transformCallback(()=>{})}); return typeof id }catch(e){ return 'denied '+String(e) }");
  check("capability: event listen (focus refresh) still allowed", listen === "number", listen);
  // --- CSP
  const violations = await ev("return window.__csp");
  check("no CSP violation during the whole session", violations.length === 0, JSON.stringify(violations));
  check("page errors: none", (await ev("return window.__errors||[]")).length === 0);
  const inline = await ev("const s=document.createElement('script'); s.textContent='window.__inlineRan=1'; document.head.appendChild(s); await new Promise(r=>setTimeout(r,200)); return window.__inlineRan===1");
  check("CSP enforced: inline script is blocked (script-src 'self')", inline === false);
  const net = await ev("try{ await fetch('https://example.com/'); return 'allowed' } catch(e){ return 'blocked' }");
  check("CSP enforced: connect-src blocks external requests", net === "blocked", net);
  const base = await ev("const b=document.createElement('base'); b.href='https://example.com/'; document.head.appendChild(b); await new Promise(r=>setTimeout(r,200)); const blocked=window.__csp.some(v=>v.startsWith('base-uri')) && document.baseURI.startsWith('http://tauri.localhost'); b.remove(); return blocked");
  check("CSP enforced: base-uri 'none'", base);
  const fontFace = await ev("return document.fonts.check('12px Geist')");
  check("bundled Geist font still loads under the CSP", fontFace);
} catch (error) {
  check("scenario aborted", false, String(error));
} finally {
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  app.kill();
  ws.close();
  process.exit(failed ? 1 : 0);
}
