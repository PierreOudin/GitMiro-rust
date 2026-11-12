import assert from "node:assert/strict";
import test from "node:test";
import { reachableCommitIds } from "../src/history-filter.ts";

test("branch filter marks only the branch tip and its loaded ancestors as reachable", () => {
  const commits = [
    { id: "main", parents: ["base"] },
    { id: "feature", parents: ["merge"] },
    { id: "merge", parents: ["base", "other"] },
    { id: "other", parents: ["base"] },
    { id: "base", parents: [] },
  ];

  assert.deepEqual(
    [...reachableCommitIds(commits, "feature")].sort(),
    ["base", "feature", "merge", "other"],
  );
});

test("branch tip stays reachable when older pages have not loaded it", () => {
  assert.deepEqual([...reachableCommitIds([], "older-tip")], ["older-tip"]);
});
