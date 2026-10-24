import assert from "node:assert/strict";
import { test } from "node:test";
import config from "../vite.config.ts";

test("the development server accepts arbitrary reverse-proxy hostnames by default", () => {
  assert.equal(config.server.allowedHosts, true);
});
