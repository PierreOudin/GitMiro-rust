// Builds GM-67 end-to-end fixtures under a temp directory (never inside the repository).
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = process.argv[2];
rmSync(root, { recursive: true, force: true });
mkdirSync(root, { recursive: true });

const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8" });
const init = (name) => {
  const dir = join(root, name);
  mkdirSync(dir);
  git(dir, "init", "-q", "-b", "main");
  for (const [k, v] of [["user.name", "GM67 Test"], ["user.email", "gm67@example.invalid"], ["commit.gpgsign", "false"], ["core.autocrlf", "false"]]) git(dir, "config", k, v);
  return dir;
};

// big: 1300 commits through fast-import, then staged + unstaged changes.
const big = init("big");
let stream = "";
for (let i = 1; i <= 1300; i++) {
  const msg = `commit number ${i}`;
  const body = `content ${i}\n`;
  stream += `commit refs/heads/main\nmark :${i}\nauthor GM67 Test <gm67@example.invalid> ${1700000000 + i} +0000\ncommitter GM67 Test <gm67@example.invalid> ${1700000000 + i} +0000\ndata ${msg.length}\n${msg}\n`;
  if (i > 1) stream += `from :${i - 1}\n`;
  stream += `M 100644 inline f.txt\ndata ${body.length}\n${body}\n`;
}
const imp = spawnSync("git", ["-C", big, "fast-import", "--quiet"], { input: stream, encoding: "utf8" });
if (imp.status !== 0) throw new Error(imp.stderr);
git(big, "reset", "-q", "--hard", "main");
git(big, "tag", "v1.0", "HEAD~10");
git(big, "branch", "feature", "HEAD~700");
writeFileSync(join(big, "f.txt"), "unstaged edit\n");
writeFileSync(join(big, "new.txt"), "staged new file\n");
git(big, "add", "new.txt");

// merge: a merge whose resolved index equals HEAD (nothing staged, MERGE_HEAD present).
const merge = init("merge");
writeFileSync(join(merge, "file.txt"), "base\n");
git(merge, "add", "."); git(merge, "commit", "-q", "-m", "base");
git(merge, "checkout", "-q", "-b", "side");
writeFileSync(join(merge, "file.txt"), "changed\n");
git(merge, "commit", "-qam", "side change");
git(merge, "checkout", "-q", "main");
writeFileSync(join(merge, "file.txt"), "changed\n");
git(merge, "commit", "-qam", "same change on main");
git(merge, "merge", "--no-commit", "--no-ff", "side");

// plain: small repo to commit in.
const plain = init("plain");
writeFileSync(join(plain, "a.txt"), "a\n");
git(plain, "add", "."); git(plain, "commit", "-q", "-m", "first");
writeFileSync(join(plain, "b.txt"), "b\n");

console.log(JSON.stringify({ big, merge, plain }));
