import { readFile, writeFile, readdir } from "node:fs/promises";
import { resolve, join } from "node:path";
import { partitionCpuIntervals, normalizeTraceEvents } from "./paint-metrics.mjs";
const dir = resolve(process.argv[2]);
const report = JSON.parse(await readFile(join(dir, "results.json"), "utf8"));
const median = values => { const rows = [...values].sort((a, b) => a - b); return rows.length % 2 ? rows[Math.floor(rows.length / 2)] : (rows[rows.length / 2 - 1] + rows[rows.length / 2]) / 2; };
const stat = values => ({ n: values.length, median: median(values), min: Math.min(...values), max: Math.max(...values) });
const role = row => row.type === "renderer" && row.name.startsWith("ThreadPoolFore") ? "renderer-workers" : row.type === "renderer" && (row.pid === row.tid || row.name === "CrRendererMain") ? "renderer-main" : row.type === "renderer" && row.name === "Compositor" ? "renderer-compositor" : row.type === "GPU" && row.name.startsWith("VizCompositor") ? "viz-compositor" : row.type === "browser" ? "browser" : "other";
const keys = [...new Set(report.records.map(row => [row.version, row.format, row.variant, row.scenario].join("/")))];
const groups = keys.map(key => {
 const rows = report.records.filter(row => [row.version, row.format, row.variant, row.scenario].join("/") === key);
 const roles = {};
 for (const name of ["renderer-main", "renderer-workers", "renderer-compositor", "viz-compositor", "browser", "other"]) roles[name] = stat(rows.map(row => row.threadCpu.threads.filter(thread => role(thread) === name).reduce((sum, thread) => sum + thread.cpuMs, 0)));
 return { key, n: rows.length, totalCpuMs: stat(rows.map(row => row.browserCpuMs)), threadCpuMs: stat(rows.map(row => row.threadCpu.totalCpuMs)), roles, allThreadBudgetsValid: rows.every(row => row.threadCpu.valid), maxProcessThreadDiscrepancyMs: Math.max(...rows.map(row => Math.abs(row.threadCpu.totalCpuMs - row.browserCpuMs))), scrollTops: rows.map(row => row.detail.scrollTop), scriptCpuMs: stat(rows.map(row => row.scriptCpuMs)), geometry: report.fixtureGeometry.filter(row => [row.version, row.format, row.variant, "scroll"].join("/") === key).map(row => row.geometry), lifecycle: rows.map(row => row.browserLifecycle).filter(Boolean) };
});
const traces = [];
for (const file of (await readdir(dir)).filter(file => file.endsWith(".trace.json"))) {
 const rawEvents = JSON.parse(await readFile(join(dir, file), "utf8")).traceEvents;
 const marks = rawEvents.filter(event => event.name === "mandara-paint-start" || event.name === "mandara-paint-end");
 const start = marks.find(event => event.name === "mandara-paint-start")?.ts;
 const end = marks.find(event => event.name === "mandara-paint-end")?.ts;
 if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) throw new Error("Missing ordered measurement marks in " + file);
 const cpu = partitionCpuIntervals(rawEvents, { start, end });
 const events = normalizeTraceEvents(rawEvents).events.filter(event => event.ph === "X" && event.ts >= start && event.ts + event.dur <= end);
 const layerData = JSON.parse(await readFile(join(dir, file.replace(".trace.json", ".layers.json")), "utf8"));
 const layers = new Map();
 for (const event of events.filter(event => event.name === "RasterTask")) {
  const data = event.args?.tileData || {};
  const id = String(data.layerId ?? "unknown");
  const row = layers.get(id) || { layerId: id, tasks: 0, inclusiveCpuMs: 0, sourceFrames: new Set() };
  row.tasks++; row.inclusiveCpuMs += (event.tdur || 0) / 1000;
  row.sourceFrames.add(data.sourceFrameNumber);
  layers.set(id, row);
 }
 const rasterOps = events.filter(event => event.name === "DisplayItemList::Raster");
 const totalOps = rasterOps.map(event => event.args?.total_op_count).filter(Number.isFinite);
 const playedOps = rasterOps.map(event => event.args?.rastered_op_count).filter(Number.isFinite);
 const counts = Object.fromEntries(["Paint", "RasterTask", "UpdateLayoutTree", "DirectRenderer::DrawFrame", "SoftwareRenderer::DoDrawQuad"].map(name => [name, events.filter(event => event.name === name).length]));
 const stages = {}; for (const thread of cpu.threads) for (const [stage, value] of Object.entries(thread.stages)) stages[stage] = (stages[stage] || 0) + value;
 const prefix = file.replace(".trace.json", "");
 const measured = report.records.find(row => [row.repetition, row.version, row.format, row.variant, row.scenario].join("-") === prefix);
 traces.push({ file, wallMs: (end - start) / 1000, cpu, stages, counts, osThreadCpuMs: measured.threadCpu.totalCpuMs, traceCpuCoverage: cpu.coveredCpuMs / measured.threadCpu.totalCpuMs, rasterDisplayLists: { calls: rasterOps.length, totalOpCount: totalOps.length ? stat(totalOps) : null, playedOpCount: playedOps.length ? stat(playedOps) : null, playedOpsSum: playedOps.reduce((sum, count) => sum + count, 0) }, rasterLayers: [...layers.values()].map(row => ({ ...row, sourceFrames: row.sourceFrames.size, mapping: layerData.layers.find(layer => layer.layerId === row.layerId) || layerData.observedLayers.find(layer => layer.layerId === row.layerId) })).sort((a, b) => b.inclusiveCpuMs - a.inclusiveCpuMs), layerData });
}
const result = { revision: report.current, environment: report.environment, protocol: report.protocol, errors: report.errors, notes: ["Thread CPU is sampled only at OS endpoints with the reported clock-tick resolution.", "Complete synchronous trace scopes are partitioned on each thread CPU clock so nested scopes cannot double-count CPU.", "Only fully contained scopes contribute to trace CPU; boundary scopes and missing CPU clocks are separately reported.", "Stage timing and command replay come from instrumented runs, not clean measurements.", "Command replay timing is wall time on an isolated software bitmap, not live frame CPU or GPU work."], groups, traces };
await writeFile(join(dir, "paint-analysis.json"), JSON.stringify(result, null, 2));
console.log(JSON.stringify({ groups: groups.map(group => ({ key: group.key, total: group.totalCpuMs, roles: Object.fromEntries(Object.entries(group.roles).map(([key, value]) => [key, value.median])), valid: group.allThreadBudgetsValid })), traces: traces.map(trace => ({ file: trace.file, stages: trace.stages, coverage: trace.traceCpuCoverage, counts: trace.counts, layers: trace.rasterLayers.map(layer => ({ id: layer.layerId, tasks: layer.tasks, cpu: layer.inclusiveCpuMs, node: layer.mapping?.node })) })) }));
