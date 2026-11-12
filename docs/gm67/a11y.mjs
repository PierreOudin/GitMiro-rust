// GM-67 accessibility smoke on the real app: AX tree names, keyboard order, focus visibility, contrast, min window.
// usage: node a11y.mjs <exe> <fixturesDir>
import { spawn } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const [exe, fixtures] = process.argv.slice(2);
const work = join(fixtures, "_a11y");
rmSync(work, { recursive: true, force: true });
mkdirSync(work, { recursive: true });
writeFileSync(join(work, "empty-gitconfig"), "");
const PORT = 9226;
const app = spawn(exe, [], { env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`, WEBVIEW2_USER_DATA_FOLDER: join(work, "wv"), GIT_CONFIG_GLOBAL: join(work, "empty-gitconfig") }, stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let url;
for (let i = 0; i < 100 && !url; i++) {
  try { url = (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).find((t) => t.type === "page")?.webSocketDebuggerUrl; } catch {}
  await sleep(200);
}
const ws = new WebSocket(url);
await new Promise((r) => (ws.onopen = r));
let id = 1; const pend = new Map();
ws.onmessage = (m) => { const j = JSON.parse(m.data); if (j.id && pend.has(j.id)) { pend.get(j.id)(j); pend.delete(j.id); } };
const send = (method, params = {}) => new Promise((r) => { const i = id++; pend.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const ev = async (e) => { const r = await send("Runtime.evaluate", { expression: `(async()=>{${e}})()`, awaitPromise: true, returnByValue: true }); if (r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails)); return r.result.result.value; };
const waitFor = async (e, t = 15000) => { const end = Date.now() + t; while (Date.now() < end) { try { if (await ev(`return !!(${e})`)) return true; } catch {} await sleep(100); } return false; };
const key = async (k, code, vk, modifiers = 0) => { for (const type of ["keyDown", "keyUp"]) await send("Input.dispatchKeyEvent", { type, key: k, code, windowsVirtualKeyCode: vk, modifiers }); };

const out = { findings: [] };
const note = (name, ok, detail) => { out.findings.push({ name, ok, detail }); console.log(`${ok ? "PASS" : "FINDING"}  ${name}${detail ? "  -> " + detail : ""}`); };

await send("Accessibility.enable");
await send("Page.enable");
await waitFor("document.body");
const big = join(fixtures, "big");
await ev(`localStorage.setItem('gitmiro.recentRepositories', ${JSON.stringify(JSON.stringify([big]))}); location.reload();`);
await sleep(600);
await waitFor("document.querySelector('.recent-repository')");

// Welcome screen keyboard order
const welcomeFocus = await ev("return document.activeElement?.id || document.activeElement?.className");
note("welcome: recent filter focused first when recents exist", welcomeFocus === "recent-filter", String(welcomeFocus));

await ev("document.querySelector('.recent-repository').click()");
await waitFor("document.querySelector('.repository-heading h1')?.textContent==='big'");
await waitFor("document.querySelectorAll('.commit-list li').length>0");

// AX tree: every interactive node must have an accessible name
const tree = (await send("Accessibility.getFullAXTree")).result.nodes;
const interactive = tree.filter((n) => !n.ignored && ["button", "textbox", "combobox", "checkbox", "link", "searchbox", "listbox"].includes(n.role?.value));
const unnamed = interactive.filter((n) => !(n.name?.value || "").trim());
note(`AX tree: ${interactive.length} interactive nodes, all named`, unnamed.length === 0, unnamed.map((n) => n.role.value).join(","));
const headings = tree.filter((n) => !n.ignored && n.role?.value === "heading").map((n) => n.name?.value);
note("AX tree: headings present", headings.length >= 4, headings.join(" | "));
const landmarks = tree.filter((n) => !n.ignored && ["main", "complementary", "region", "banner"].includes(n.role?.value)).map((n) => `${n.role.value}:${n.name?.value}`);
note("AX tree: landmarks/regions exposed", landmarks.length >= 4, landmarks.join(" | "));

// Tab traversal order and focus visibility
await ev("document.activeElement.blur(); window.scrollTo(0,0)");
const order = [];
const invisible = [];
for (let i = 0; i < 40; i++) {
  await key("Tab", "Tab", 9);
  const info = await ev(`const e=document.activeElement; const cs=getComputedStyle(e); return {tag:e.tagName.toLowerCase(), label:(e.getAttribute('aria-label')||e.id||e.textContent||'').trim().slice(0,28), outline:cs.outlineStyle!=='none'&&parseFloat(cs.outlineWidth)>0, shadow:cs.boxShadow!=='none', bg:cs.backgroundColor, ring:e.matches(':focus-visible')}`);
  order.push(`${info.tag}:${info.label}`);
  if (!info.outline && !info.shadow) invisible.push(`${info.tag}:${info.label}`);
}
console.log("tab order (first 40):\n  " + order.join("\n  "));
note("keyboard: every tab stop shows an outline or ring", invisible.length === 0, invisible.slice(0, 8).join(" ; "));
note("keyboard: the history list scroller is not trapped (focus leaves .commit-history)", !(await ev("return !!document.activeElement.closest('.commit-history')")));

// Contrast of key text colors against their effective background
const contrast = await ev(`
  const parse=c=>c.match(/[\\d.]+/g).map(Number);
  const lum=([r,g,b])=>{const f=v=>{v/=255;return v<=0.03928?v/12.92:Math.pow((v+0.055)/1.055,2.4)};return 0.2126*f(r)+0.7152*f(g)+0.0722*f(b)};
  const bgOf=e=>{while(e){const c=getComputedStyle(e).backgroundColor; const p=parse(c); if(p.length<4||p[3]>0.5) return p.slice(0,3); e=e.parentElement} return [255,255,255]};
  const rows=[];
  for(const [name,sel] of [['commit subject','.commit-subject'],['commit meta','.commit-meta'],['repo path','.repository-path'],['status kind','.status-kind'],['branch label','.branch-label'],['status count','.status-count'],['ref item','.reference-item'],['heading h2','.reference-group h2']]){
    const e=document.querySelector(sel); if(!e) continue;
    const fg=parse(getComputedStyle(e).color).slice(0,3), bg=bgOf(e);
    const l1=lum(fg), l2=lum(bg); const ratio=(Math.max(l1,l2)+0.05)/(Math.min(l1,l2)+0.05);
    rows.push({name, ratio:Math.round(ratio*100)/100, size:getComputedStyle(e).fontSize});
  }
  return {scheme:matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light', rows};`);
console.log("contrast (" + contrast.scheme + "):", JSON.stringify(contrast.rows));
const low = contrast.rows.filter((r) => r.ratio < 4.5);
note("contrast: all sampled text >= 4.5:1", low.length === 0, low.map((r) => `${r.name} ${r.ratio}:1 @${r.size}`).join(" ; "));

// Dark/light both
for (const scheme of ["light", "dark"]) {
  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] });
  await sleep(200);
  const c = await ev(`
    const parse=c=>c.match(/[\\d.]+/g).map(Number);
    const lum=([r,g,b])=>{const f=v=>{v/=255;return v<=0.03928?v/12.92:Math.pow((v+0.055)/1.055,2.4)};return 0.2126*f(r)+0.7152*f(g)+0.0722*f(b)};
    const bgOf=e=>{while(e){const c=getComputedStyle(e).backgroundColor; const p=parse(c); if(p.length<4||p[3]>0.5) return p.slice(0,3); e=e.parentElement} return [255,255,255]};
    const worst=[]; for(const e of document.querySelectorAll('.commit-meta,.repository-path,.status-kind,.muted,.reference-item,.commit-subject,.status-action,.branch-label,.ref-chip,.button-primary')){const fg=parse(getComputedStyle(e).color).slice(0,3),bg=bgOf(e);const l1=lum(fg),l2=lum(bg);worst.push((Math.max(l1,l2)+0.05)/(Math.min(l1,l2)+0.05))}
    return Math.round(Math.min(...worst)*100)/100`);
  note(`contrast (${scheme} scheme): lowest ratio over common text classes >= 4.5`, c >= 4.5, `${c}:1`);
}

// Minimum window size: no horizontal page overflow at 800x560
await send("Emulation.setDeviceMetricsOverride", { width: 800, height: 560, deviceScaleFactor: 1, mobile: false });
await sleep(300);
const overflow = await ev("return {h: document.documentElement.scrollWidth > document.documentElement.clientWidth, w: document.documentElement.scrollWidth, c: document.documentElement.clientWidth}");
note("layout at the 800x560 minimum window: no page-level horizontal overflow", overflow.h === false, JSON.stringify(overflow));
await send("Emulation.setDeviceMetricsOverride", { width: 1100, height: 720, deviceScaleFactor: 1.5, mobile: false });
await sleep(300);
const overflow15 = await ev("return document.documentElement.scrollWidth > document.documentElement.clientWidth");
note("layout at 150% scale (1100x720 css px): no page-level horizontal overflow", overflow15 === false);

const failed = out.findings.filter((f) => !f.ok).length;
console.log(`\n${out.findings.length - failed}/${out.findings.length} OK, ${failed} finding(s)`);
app.kill(); ws.close(); process.exit(0);
