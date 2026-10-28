import { readFile, readdir } from "node:fs/promises";

export function measurementKey(row, protocol = {}) {
  return [row.version, row.format, row.variant, row.scenario, ...(protocol.idleSegments > 1 && row.idleSegment ? ["segment-" + row.idleSegment] : [])].join("/");
}

export function parseThreadStat(text) {
  const end = text.lastIndexOf(")");
  const fields = text.slice(end + 2).trim().split(/\s+/);
  if (end < 0 || fields.length < 20) throw new Error("Invalid Linux task stat");
  return { tid: Number(text.slice(0, text.indexOf(" "))), name: text.slice(text.indexOf("(") + 1, end), ticks: Number(fields[11]) + Number(fields[12]), startTicks: Number(fields[19]) };
}

export async function readThreadSnapshot(processes, hz) {
  const started = performance.now();
  const bootTicks = Number((await readFile("/proc/uptime", "utf8")).split(" ")[0]) * hz;
  const errors = [];
  const rows = (await Promise.all(processes.map(async process => {
    try {
      const tids = await readdir("/proc/" + process.id + "/task");
      return (await Promise.all(tids.map(async tid => {
        try { return { pid: process.id, type: process.type, ...parseThreadStat(await readFile("/proc/" + process.id + "/task/" + tid + "/stat", "utf8")) }; }
        catch (error) { errors.push({ pid: process.id, tid, code: error.code }); return null; }
      }))).filter(Boolean);
    } catch (error) { errors.push({ pid: process.id, code: error.code }); return []; }
  }))).flat();
  return { bootTicks, rows, errors, readMs: performance.now() - started };
}

export function diffThreadSnapshots(before, after, hz) {
  const old = new Map(before.rows.map(row => [row.pid + ":" + row.tid, row]));
  const threads = [], born = [], invalid = [];
  for (const row of after.rows) {
    const key = row.pid + ":" + row.tid;
    const previous = old.get(key);
    old.delete(key);
    const same = previous?.startTicks === row.startTicks;
    if (previous && !same) invalid.push({ reason: "reused tid", pid: row.pid, tid: row.tid });
    if (!same && row.startTicks < before.bootTicks - 1) invalid.push({ reason: "newly observed pre-existing task", pid: row.pid, tid: row.tid });
    if (!same) born.push({ pid: row.pid, tid: row.tid });
    const delta = row.ticks - (same ? previous.ticks : 0);
    if (delta < 0) invalid.push({ reason: "counter decreased", pid: row.pid, tid: row.tid });
    threads.push({ pid: row.pid, tid: row.tid, type: row.type, name: row.name, cpuMs: delta * 1000 / hz });
  }
  const exited = [...old.values()].map(({ pid, tid }) => ({ pid, tid }));
  const groups = {};
  for (const row of threads) {
    const role = row.type === "renderer" && row.name.startsWith("ThreadPoolFore") ? "renderer-workers" : row.type === "renderer" && (row.tid === row.pid || row.name === "CrRendererMain") ? "renderer-main" : row.type === "renderer" && row.name === "Compositor" ? "renderer-compositor" : row.type === "GPU" && row.name.startsWith("VizCompositor") ? "viz-compositor" : row.type === "browser" ? "browser" : "other";
    groups[role] = (groups[role] || 0) + row.cpuMs;
  }
  return { hz, resolutionMs: 1000 / hz, totalCpuMs: threads.reduce((sum, row) => sum + row.cpuMs, 0), groups, threads: threads.filter(row => row.cpuMs > 0).sort((a, b) => b.cpuMs - a.cpuMs), valid: !invalid.length && !exited.length && !before.errors.length && !after.errors.length, born, exited, invalid, readErrors: [...before.errors, ...after.errors], endpointReadMs: [before.readMs, after.readMs] };
}

// Include reaped-child ticks at each live ancestor, so CPU spent before the
// scroll window or in exited startup children is not silently discarded.
export async function readBrowserLifecycle(rootPid, hz) {
  const rows = [], errors = [], seen = new Set();
  async function visit(pid) {
    if (seen.has(pid)) return;
    seen.add(pid);
    try {
      const tids = await readdir("/proc/" + pid + "/task");
      const children = new Set((await Promise.all(tids.map(async tid => {
        try { return (await readFile("/proc/" + pid + "/task/" + tid + "/children", "utf8")).trim().split(/\s+/).filter(Boolean).map(Number); }
        catch (error) { if (error.code !== "ENOENT") errors.push({ pid, code: error.code }); return []; }
      }))).flat());
      for (const child of children) await visit(child);
      const text = await readFile("/proc/" + pid + "/stat", "utf8");
      const fields = text.slice(text.lastIndexOf(")") + 2).trim().split(/\s+/);
      const stat = parseThreadStat(text);
      const row = { pid, name: stat.name, ownCpuMs: stat.ticks * 1000 / hz, reapedChildrenCpuMs: (Number(fields[13]) + Number(fields[14])) * 1000 / hz };
      try {
        const memory = await readFile("/proc/" + pid + "/smaps_rollup", "utf8");
        const values = Object.fromEntries([...memory.matchAll(/^([A-Za-z_]+):\s+(\d+) kB/gm)].map(match => [match[1], Number(match[2]) * 1024]));
        row.pssBytes = values.Pss; row.rssBytes = values.Rss;
      } catch (error) { row.memoryError = error.code; }
      rows.push(row);
    } catch (error) { errors.push({ pid, code: error.code }); }
  }
  const started = performance.now();
  await visit(rootPid);
  return { rootPid, scope: "Cumulative browser process tree CPU from startup, including reaped children; endpoint memory is not a peak", cpuMs: rows.reduce((sum, row) => sum + row.ownCpuMs + row.reapedChildrenCpuMs, 0), pssBytes: rows.reduce((sum, row) => sum + (row.pssBytes || 0), 0), memoryComplete: rows.length > 0 && rows.every(row => Number.isFinite(row.pssBytes)), valid: rows.length > 0 && !errors.length, readMs: performance.now() - started, rows, errors };
}

function cpuStage(event) {
  const name = event.name;
  if (/DisplayItemList::Raster/.test(name)) return "raster-display-list";
  if (/RasterTask|RasterizerTask|RasterBuffer.*Playback|RasterSource::/.test(name)) return "raster-setup";
  if (/DecodeImage|ImageDecode/.test(name)) return "image-decode";
  if (name === "SoftwareRenderer::DoDrawQuad") return "software-quads";
  if (/DirectRenderer::DrawRenderPass|SoftwareRenderer::/.test(name)) return "render-pass";
  if (/ShapeText|performLayout|^Layout$|UpdateLayout$/.test(name)) return "layout-text-shaping";
  if (/recalcStyle|^UpdateLayoutTree$/.test(name)) return "style";
  if (/PaintArtifactCompositor|Layerize|CompositingInputs|^Commit$|UpdateLayers/.test(name)) return "layerize-commit";
  if (/PrePaint/.test(name)) return "prepaint";
  if (/^Paint$|PaintController::/.test(name)) return "paint-recording";
  if (/V8[.]GC|GarbageCollect/.test(name)) return "garbage-collection";
  if (/FunctionCall|RunMicrotasks|EvaluateScript/.test(name)) return "javascript";
  return null;
}

export function normalizeTraceEvents(events) {
  const complete = events.filter(event => event.ph !== "B" && event.ph !== "E");
  const stacks = new Map();
  let unpaired = 0;
  for (const event of events.filter(event => event.ph === "B" || event.ph === "E").sort((a, b) => a.ts - b.ts)) {
    const key = event.pid + ":" + event.tid;
    if (!stacks.has(key)) stacks.set(key, []);
    const stack = stacks.get(key);
    if (event.ph === "B") stack.push(event);
    else {
      const begin = stack.pop();
      if (!begin) { unpaired++; continue; }
      complete.push({ ...begin, ph: "X", dur: event.ts - begin.ts, tdur: Number.isFinite(begin.tts) && Number.isFinite(event.tts) ? event.tts - begin.tts : undefined, args: { ...begin.args, ...event.args } });
    }
  }
  return { events: complete, unpaired: unpaired + [...stacks.values()].reduce((sum, stack) => sum + stack.length, 0) };
}

// Partition the per-thread CPU clock, not wall time. Every covered CPU interval
// is assigned once even when categories and synchronous scopes are nested.
export function partitionCpuIntervals(events, { start = -Infinity, end = Infinity } = {}) {
  const normalized = normalizeTraceEvents(events);
  events = normalized.events;
  const threads = new Map(), names = new Map(), processNames = new Map();
  for (const event of events) if (event.ph === "M") {
    if (event.name === "thread_name") names.set(event.pid + ":" + event.tid, event.args.name);
    if (event.name === "process_name") processNames.set(event.pid, event.args.name);
  }
  let missingCpuEvents = 0, boundaryCpuUpperUs = 0, rejectedEvents = 0;
  for (const [index, event] of events.entries()) {
    if (event.ph !== "X" || event.dur < 0) continue;
    if (event.ts + event.dur <= start || event.ts >= end) continue;
    if (!Number.isFinite(event.tts) || !Number.isFinite(event.tdur)) { missingCpuEvents++; continue; }
    if (event.tdur < 0) { rejectedEvents++; continue; }
    if (event.ts < start || event.ts + event.dur > end) { boundaryCpuUpperUs += event.tdur; continue; }
    if (!event.tdur) continue;
    const key = event.pid + ":" + event.tid;
    if (!threads.has(key)) threads.set(key, []);
    threads.get(key).push({ ...event, index });
  }
  const output = [];
  for (const [key, rows] of threads) {
    const endpoints = rows.flatMap(event => [{ time: event.tts, begin: true, event }, { time: event.tts + event.tdur, begin: false, event }]).sort((a, b) => a.time - b.time || Number(a.begin) - Number(b.begin));
    const active = new Map(), stages = {}, self = new Map();
    let coveredUs = 0, previous = endpoints[0].time;
    for (const point of endpoints) {
      const delta = point.time - previous;
      if (delta > 0 && active.size) {
        const ordered = [...active.values()].sort((a, b) => a.dur - b.dur || b.index - a.index);
        const leaf = ordered[0];
        const thread = names.get(key) || key;
        const stage = ordered.map(event => cpuStage(event)).find(Boolean) || (thread === "Compositor" ? "compositor-other" : thread === "VizCompositorThread" ? "viz-other" : "other");
        stages[stage] = (stages[stage] || 0) + delta / 1000;
        self.set(leaf.name, (self.get(leaf.name) || 0) + delta / 1000);
        coveredUs += delta;
      }
      if (point.begin) active.set(point.event.index, point.event); else active.delete(point.event.index);
      previous = point.time;
    }
    output.push({ pid: rows[0].pid, tid: rows[0].tid, process: processNames.get(rows[0].pid), thread: names.get(key) || key, coveredCpuMs: coveredUs / 1000, stages, topSelf: [...self].sort((a, b) => b[1] - a[1]).slice(0, 12).map(([name, cpuMs]) => ({ name, cpuMs })) });
  }
  return { threads: output.sort((a, b) => b.coveredCpuMs - a.coveredCpuMs), coveredCpuMs: output.reduce((sum, row) => sum + row.coveredCpuMs, 0), unpairedSynchronousEvents: normalized.unpaired, missingCpuEvents, boundaryCpuUpperMs: boundaryCpuUpperUs / 1000, rejectedEvents };
}
