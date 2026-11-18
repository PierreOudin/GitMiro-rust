import assert from "node:assert/strict";
import test from "node:test";
import { cloneDestination, cloneFolderName } from "../src/clone.ts";

test("the folder name comes from the last segment of any supported source", () => {
  assert.equal(cloneFolderName("https://github.com/owner/GitMiro.git"), "GitMiro");
  assert.equal(cloneFolderName("https://github.com/owner/GitMiro/"), "GitMiro");
  assert.equal(cloneFolderName("git@github.com:owner/repo.git"), "repo");
  assert.equal(cloneFolderName("git@host:repo.git"), "repo");
  assert.equal(cloneFolderName("ssh://git@host:22/team/project"), "project");
  assert.equal(cloneFolderName("C:\\Repos\\Mon dépôt\\"), "Mon dépôt");
  assert.equal(cloneFolderName("  /srv/git/app.GIT  "), "app");
  assert.equal(cloneFolderName("D:\\WS\\proj\\.git"), "proj");
  assert.equal(cloneFolderName("/srv/proj/.git/"), "proj");
  assert.equal(cloneFolderName(""), "");
});

test("the destination joins parent and folder with the parent's own separator", () => {
  assert.equal(cloneDestination("C:\\Repos", "app"), "C:\\Repos\\app");
  assert.equal(cloneDestination("C:\\", "app"), "C:\\app");
  assert.equal(cloneDestination("/Users/me/repos/", " app "), "/Users/me/repos/app");
  assert.equal(cloneDestination("/", "app"), "/app");
  assert.equal(cloneDestination(" C:\\Repos \n", "app"), "C:\\Repos\\app");
});

test("a destination needs a parent and a single folder name", () => {
  for (const name of ["", "  ", ".", "..", "a/b", "a\\b"]) assert.equal(cloneDestination("C:\\Repos", name), null);
  assert.equal(cloneDestination(" ", "app"), null);
});
