// GM-66 Tauri frontend latency scenarios over the WebView2 DevTools protocol.
// Usage: node tauri-cdp.mjs <port> <repoPath> <runs=10> <out.json>
// The app must be started with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>.
// All times are performance.now() deltas inside the page; "paint" = after two animation frames.
import { writeFileSync } from "node:fs";

const [port, repo, runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 10);
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); }
};
const send = (method, params = {}) => new Promise((res, rej) => {
  const i = ++id;
  pending.set(i, (d) => (d.error ? rej(new Error(JSON.stringify(d.error))) : res(d.result)));
  ws.send(JSON.stringify({ id: i, method, params }));
});
// Evaluate an async expression in the page and return its JSON value.
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression: `(async()=>{${expression}})()`, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const HELPERS = `
const waitFor=(fn,timeout=30000)=>new Promise((res,rej)=>{const t0=performance.now();const tick=()=>{let v;try{v=fn()}catch{}if(v)return res(v);if(performance.now()-t0>timeout)return rej(new Error('timeout'));requestAnimationFrame(tick)};tick()});
const paint=()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(()=>r(performance.now()))));
performance.clearResourceTimings();
const q=(s)=>document.querySelector(s);const qa=(s)=>[...document.querySelectorAll(s)];
const ipc=(cmd)=>{const x=performance.getEntriesByType('resource').filter(e=>e.name.endsWith('/'+cmd));return x.length?x[x.length-1].duration:null};
`;

const results = {};
const add = (name, row) => (results[name] ??= []).push(row);

for (let run = 0; run < runs; run++) {
  // Fresh welcome screen each repetition; the recent-repository list lives in localStorage.
  await evaluate(`localStorage.setItem("gitmiro.recentRepositories", ${JSON.stringify(JSON.stringify([repo]))}); location.reload(); await new Promise(r=>setTimeout(r,50)); return 1`).catch(() => {});
  await sleep(1500);

  // 1. open repository -> history visible/painted, and status entries visible.
  add("open_repository", JSON.parse(await evaluate(`${HELPERS}
    await waitFor(()=>q('.recent-repository'));
    const t0=performance.now();q('.recent-repository').click();
    await waitFor(()=>q('.commit-list li'));const tHist=await paint();
    await waitFor(()=>qa('[aria-label^="View diff for"]').length>=8);const tStatus=await paint();
    return JSON.stringify({history_painted_ms:tHist-t0,status_painted_ms:tStatus-t0,ipc_open_repository_ms:ipc('open_repository'),ipc_refresh_status_ms:ipc('refresh_repository_status'),commits:qa('.commit-list li').length});`)));

  // 2. branch filter change (All -> a feature branch -> All).
  add("branch_filter", JSON.parse(await evaluate(`${HELPERS}
    const sel=q('#history-branch-filter');const opts=[...sel.options].filter(o=>o.value);const target=opts.find(o=>/feature/.test(o.textContent))||opts[0];
    const set=(v)=>{Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(sel,v);sel.dispatchEvent(new Event('change',{bubbles:true}))};
    const before=qa('.commit-dimmed').length;
    let t0=performance.now();set(target.value);await waitFor(()=>qa('.commit-dimmed').length!==before);const to=await paint()-t0;
    t0=performance.now();set('');await waitFor(()=>qa('.commit-dimmed').length===0);const back=await paint()-t0;
    return JSON.stringify({select_feature_painted_ms:to,clear_painted_ms:back});`)));

  // 3. scroll the full page by wheel-sized steps; frame intervals via rAF; infinite loading kicks in near the end.
  add("scroll_history", JSON.parse(await evaluate(`${HELPERS}
    const sc=q('.commit-history');sc.scrollTop=0;await paint();
    const deltas=[];let last=performance.now();const t0=last;let frames=0;
    await new Promise(res=>{const step=()=>{const n=performance.now();deltas.push(n-last);last=n;frames++;sc.scrollTop+=160;
      if(frames<240&&sc.scrollTop+sc.clientHeight<sc.scrollHeight-2)requestAnimationFrame(step);else res()};requestAnimationFrame(step)});
    await paint();deltas.shift();
    const s=[...deltas].sort((a,b)=>a-b);const pct=p=>s[Math.min(s.length-1,Math.ceil(p*s.length)-1)];
    return JSON.stringify({frames:deltas.length,frame_p50_ms:pct(.5),frame_p95_ms:pct(.95),frame_max_ms:s[s.length-1],over_25ms:deltas.filter(d=>d>25).length,scrolled_px:sc.scrollTop,total_ms:performance.now()-t0});`)));

  // 3b. load older commits (second 500-commit page).
  add("load_more", JSON.parse(await evaluate(`${HELPERS}
    const n0=qa('.commit-list li').length;const b=q('.load-more');const t0=performance.now();b.click();
    await waitFor(()=>qa('.commit-list li').length>n0);const t=await paint()-t0;
    return JSON.stringify({load_more_painted_ms:t,ipc_load_more_commits_ms:ipc('load_more_commits'),commits:qa('.commit-list li').length});`)));

  // 4. file diff (unstaged, tracked) open -> painted, then back.
  add("file_diff", JSON.parse(await evaluate(`${HELPERS}
    await paint();
    const b=q('[aria-label="View diff for src/file0.txt"]');const t0=performance.now();b.click();
    await waitFor(()=>q('.file-diff-view')&&!q('.file-diff-view [role=status]')&&q('.file-diff-view').textContent.includes('unstaged edit 0'));const t=await paint()-t0;
    const ipcMs=ipc('get_file_diff');q('.file-diff-heading button').click();await waitFor(()=>q('.commit-list li'));await paint();
    return JSON.stringify({diff_painted_ms:t,ipc_get_file_diff_ms:ipcMs});`)));

  // 5. status refresh: direct IPC round trip plus stage/unstage of an untracked file with UI update.
  add("status_refresh", JSON.parse(await evaluate(`${HELPERS}
    const path=q('.repository-path').textContent;const t0=performance.now();
    await window.__TAURI_INTERNALS__.invoke('refresh_repository_status',{path});const ipcMs=performance.now()-t0;
    const stageBtn=q('[aria-label="Stage untracked0.txt"]');const s0=performance.now();stageBtn.click();
    await waitFor(()=>q('[aria-label="Unstage untracked0.txt"]'));const stageMs=await paint()-s0;
    const u0=performance.now();q('[aria-label="Unstage untracked0.txt"]').click();
    await waitFor(()=>q('[aria-label="Stage untracked0.txt"]'));const unstageMs=await paint()-u0;
    return JSON.stringify({ipc_refresh_status_ms:ipcMs,stage_painted_ms:stageMs,unstage_painted_ms:unstageMs});`)));
}

writeFileSync(out, JSON.stringify(results, null, 2));
console.log(`wrote ${out}`);
ws.close();
