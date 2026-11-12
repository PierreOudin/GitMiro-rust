import assert from "node:assert/strict";
import test from "node:test";
import { isNearEnd } from "../src/scroll.ts";

test("history loads the next page only when its own scroll container is near the end", () => {
  // The history list scrolls inside .commit-history, so the window itself never scrolls (GM-67).
  assert.equal(isNearEnd({ scrollHeight: 25000, scrollTop: 0, clientHeight: 600 }), false);
  assert.equal(isNearEnd({ scrollHeight: 25000, scrollTop: 24300, clientHeight: 600 }), true);
  assert.equal(isNearEnd({ scrollHeight: 25000, scrollTop: 24201, clientHeight: 600 }), true);
  assert.equal(isNearEnd({ scrollHeight: 25000, scrollTop: 24200, clientHeight: 600 }), false);
});

test("a list that does not overflow counts as being at its end", () => {
  assert.equal(isNearEnd({ scrollHeight: 300, scrollTop: 0, clientHeight: 600 }), true);
});
