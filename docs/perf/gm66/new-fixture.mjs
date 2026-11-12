// Deterministic benchmark fixture for GM-66: node new-fixture.mjs <dir> [commits=1000]
// Same commit count => same HEAD hash on every machine (fixed dates, authors, content).
// The worktree is then dirtied (3 staged, 5 unstaged, 2 untracked files) for status/diff scenarios.
import { spawnSync } from "node:child_process";
import { mkdirSync, readdirSync, writeFileSync, appendFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const [dir, countArg] = process.argv.slice(2);
const total = Number(countArg ?? 1000);
if (!dir || !(total >= 10)) throw new Error("usage: node new-fixture.mjs <empty-dir> [commits>=10]");
if (existsSync(dir) && readdirSync(dir).length) throw new Error(`${dir} must be empty`);
mkdirSync(dir, { recursive: true });

const git = (args, input) => {
  const r = spawnSync("git", args, { cwd: dir, input, encoding: "utf8", maxBuffer: 1 << 28 });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")}: ${r.stderr}`);
  return r.stdout.trim();
};
git(["init", "-q", "-b", "main"]);
for (const [k, v] of [["core.autocrlf", "false"], ["user.name", "Bench"], ["user.email", "bench@example.invalid"]]) git(["config", k, v]);

const FILES = 50;
const body = (i) => Array.from({ length: 40 }, (_, l) => `line ${l} of revision ${i} ${"x".repeat((i * 7 + l * 3) % 40)}`).join("\n") + "\n";
let stream = "";
let mark = 0;
const person = "Bench <bench@example.invalid>";
const commit = (ref, i, from, merge, msg) => {
  mark++;
  const when = 1700000000 + i * 3600;
  const data = `${msg}\n`;
  stream += `commit ${ref}\nmark :${mark}\ncommitter ${person} ${when} +0000\ndata ${Buffer.byteLength(data)}\n${data}`;
  if (from) stream += `from ${from}\n`;
  if (merge) stream += `merge ${merge}\n`;
  const f = `src/file${i % FILES}.txt`;
  const content = body(i);
  stream += `M 100644 inline ${f}\ndata ${Buffer.byteLength(content)}\n${content}\n`;
  return `:${mark}`;
};

// Quirk kept on purpose so every benchmark HEAD hash stays reproducible: the ref name is evaluated before n++, so each of the
// 3 side commits of a group gets its own one-commit branch (b25, b26, b27) instead of one 3-commit branch.
let n = 0;
let main = commit("refs/heads/main", n++, null, null, "initial commit");
while (n < total) {
  if (n % 25 === 0 && n + 5 <= total) {
    let side = main;
    for (let k = 0; k < 3; k++) side = commit(`refs/heads/feature/b${n}`, n++, k ? side : main, null, `feature ${n} work ${k}`);
    main = commit("refs/heads/main", n++, main, side, `merge feature/b${n - 4}`);
  } else {
    main = commit("refs/heads/main", n++, main, null, `change ${n} in file ${n % FILES}`);
  }
  if (n % 100 === 0) stream += `reset refs/tags/v${n / 100}\nfrom ${main}\n\n`;
}
git(["fast-import", "--quiet"], stream);
git(["checkout", "-q", "-f", "main"]);

for (let i = 0; i < 5; i++) appendFileSync(join(dir, `src/file${i}.txt`), `unstaged edit ${i}\n`);
for (let i = 10; i < 13; i++) appendFileSync(join(dir, `src/file${i}.txt`), `staged edit ${i}\n`);
git(["add", "src/file10.txt", "src/file11.txt", "src/file12.txt"]);
for (let i = 0; i < 2; i++) writeFileSync(join(dir, `untracked${i}.txt`), `untracked ${i}\n`);

console.log(JSON.stringify({ dir, commits: git(["rev-list", "--count", "--all"]), head: git(["rev-parse", "HEAD"]), refs: git(["for-each-ref", "--format=%(refname)"]).split("\n").length }));
