import test from "node:test";
import assert from "node:assert/strict";
import { parseThreadStat, diffThreadSnapshots, partitionCpuIntervals, normalizeTraceEvents } from "./paint-metrics.mjs";
const event = (name, tts, tdur, ts = tts, dur = tdur) => ({ ph: "X", pid: 1, tid: 2, name, tts, tdur, ts, dur });

test("Linux task stat handles spaces and parentheses in task names", () => {
  const fields = Array(20).fill("0"); fields[0] = "R"; fields[11] = "120"; fields[12] = "30"; fields[19] = "500";
  assert.deepEqual(parseThreadStat("42 (worker (odd) name) " + fields.join(" ")), { tid: 42, name: "worker (odd) name", ticks: 150, startTicks: 500 });
});
test("task counters are converted with the measured clock-tick frequency", () => {
  const row = { pid: 1, tid: 1, type: "renderer", name: "CrRendererMain", startTicks: 50, ticks: 20 };
  const before = { bootTicks: 100, rows: [row], errors: [], readMs: 1 };
  const result = diffThreadSnapshots(before, { ...before, rows: [{ ...row, ticks: 23 }] }, 100);
  assert.equal(result.totalCpuMs, 30); assert.equal(result.groups["renderer-main"], 30); assert.equal(result.valid, true);
});
test("exited threads make the OS CPU budget incomplete", () => {
  const before = { bootTicks: 100, rows: [{ pid: 1, tid: 2, startTicks: 50, ticks: 10 }], errors: [], readMs: 1 };
  assert.equal(diffThreadSnapshots(before, { ...before, rows: [] }, 100).valid, false);
});
test("nested trace scopes are not double-counted", () => {
  const result = partitionCpuIntervals([event("FunctionCall", 100, 80, 0, 200), event("UpdateLayoutTree", 120, 20, 30, 40)]);
  assert.equal(result.coveredCpuMs, 0.08);
  assert.equal(result.threads[0].stages.javascript, 0.06);
  assert.equal(result.threads[0].stages.style, 0.02);
});
test("partially overlapping or identical scopes still partition the CPU union", () => {
  const result = partitionCpuIntervals([event("Paint", 0, 10, 0, 15), event("Paint", 5, 10, 8, 15), event("Paint", 5, 10, 8, 15)]);
  assert.equal(result.coveredCpuMs, 0.015);
  assert.equal(result.threads[0].stages["paint-recording"], 0.015);
});
test("wall time is never substituted for missing thread CPU", () => {
  const noCpu = { ph: "X", name: "Paint", pid: 1, tid: 2, ts: 0, dur: 5000000 };
  const result = partitionCpuIntervals([noCpu, event("Paint", 10, 10, 1, 10)]);
  assert.equal(result.coveredCpuMs, 0.01); assert.equal(result.missingCpuEvents, 1);
});
test("CPU clocks of different threads are independent", () => {
  const result = partitionCpuIntervals([event("Paint", 0, 10), { ...event("Paint", 0, 10), tid: 3 }]);
  assert.equal(result.coveredCpuMs, 0.02);
});
test("synchronous begin/end scopes preserve CPU clocks and end arguments", () => {
  const result = normalizeTraceEvents([{ ph: "B", name: "Paint", pid: 1, tid: 2, ts: 10, tts: 5, args: { total_op_count: 4 } }, { ph: "E", pid: 1, tid: 2, ts: 30, tts: 15, args: { rastered_op_count: 2 } }]);
  assert.equal(result.unpaired, 0); assert.equal(result.events[0].tdur, 10); assert.equal(result.events[0].dur, 20);
  assert.equal(result.events[0].args.rastered_op_count, 2);
  assert.equal(partitionCpuIntervals(result.events).coveredCpuMs, 0.01);
});
test("unpaired synchronous events are reported", () => {
  const result = normalizeTraceEvents([{ ph: "E", pid: 1, tid: 2, ts: 10 }, { ph: "B", pid: 1, tid: 2, ts: 20 }]);
  assert.equal(result.unpaired, 2);
});
test("main renderer threads need not expose a Chrome-specific task name", () => {
  const row = { pid: 1, tid: 1, type: "renderer", name: "chromium", startTicks: 50, ticks: 20 };
  const before = { bootTicks: 100, rows: [row], errors: [], readMs: 1 };
  assert.equal(diffThreadSnapshots(before, { ...before, rows: [{ ...row, ticks: 23 }] }, 100).groups["renderer-main"], 30);
});
test("reused task ids invalidate counters even when the replacement is new", () => {
  const row = { pid: 1, tid: 2, type: "renderer", name: "worker", startTicks: 50, ticks: 20 };
  const before = { bootTicks: 100, rows: [row], errors: [], readMs: 1 };
  assert.equal(diffThreadSnapshots(before, { ...before, rows: [{ ...row, ticks: 3, startTicks: 101 }] }, 100).valid, false);
});
test("boundary-crossing scopes are flagged instead of proportionally inventing CPU", () => {
  const result = partitionCpuIntervals([event("Paint", 0, 10, 0, 20), event("Paint", 20, 5, 6, 5)], { start: 5, end: 15 });
  assert.equal(result.coveredCpuMs, 0.005); assert.equal(result.boundaryCpuUpperMs, 0.01);
});
