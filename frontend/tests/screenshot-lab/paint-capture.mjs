import { writeFile } from "node:fs/promises";
import { join } from "node:path";

// These diagnostic snapshots run after the measured interval. Command replay
// reports elapsed wall time, not CPU time or the live compositor's tile cost.
export async function beginLayerCapture(cdp) {
  let current = [];
  const observed = new Map(), paints = [];
  const onTree = ({ layers }) => { current = layers || []; for (const layer of current) observed.set(layer.layerId, layer); };
  const onPaint = event => paints.push(event);
  cdp.on("LayerTree.layerTreeDidChange", onTree);
  cdp.on("LayerTree.layerPainted", onPaint);
  await cdp.send("DOM.enable");
  await cdp.send("LayerTree.enable");
  return async function finish(output, prefix, commands) {
    const layers = [];
    try {
      for (const layer of current) {
        const row = { ...layer };
        if (layer.backendNodeId) try {
          const { node } = await cdp.send("DOM.describeNode", { backendNodeId: layer.backendNodeId });
          row.node = { name: node.nodeName, attributes: node.attributes, documentURL: node.documentURL };
        } catch (error) { row.nodeError = String(error); }
        try { row.reasons = await cdp.send("LayerTree.compositingReasons", { layerId: layer.layerId }); } catch (error) { row.reasonsError = String(error); }
        if (commands && layer.drawsContent && layer.width > 0 && layer.height > 0) {
          let snapshotId;
          try {
            ({ snapshotId } = await cdp.send("LayerTree.makeSnapshot", { layerId: layer.layerId }));
            const { commandLog } = await cdp.send("LayerTree.snapshotCommandLog", { snapshotId });
            row.commandCounts = {};
            for (const command of commandLog) row.commandCounts[command.method] = (row.commandCounts[command.method] || 0) + 1;
            // ProfileSnapshot allocates the full snapshot bitmap before applying
            // clipRect. Never allocate an entire long chapter for this probe.
            if (layer.width * layer.height <= 12000000) {
              const { timings } = await cdp.send("LayerTree.profileSnapshot", { snapshotId, minRepeatCount: 5, minDuration: 0.02 });
              row.replay = { unit: "wall milliseconds per standalone replay, not live CPU", repetitions: timings.length, aligned: timings.every(values => values.length === commandLog.length) };
              if (row.replay.aligned) {
                const byMethod = {};
                for (let i = 0; i < commandLog.length; i++) {
                  const method = commandLog[i].method;
                  byMethod[method] = (byMethod[method] || 0) + timings.reduce((sum, values) => sum + values[i], 0) * 1000 / timings.length;
                }
                row.replay.byMethodMs = byMethod;
              }
              await writeFile(join(output, prefix + ".layer-" + layer.layerId + ".commands.json"), JSON.stringify({ layer, commandLog, timings }));
            } else {
              row.replay = { skipped: "Full snapshot exceeds 48 MB bitmap guard; clipping does not bound allocation" };
              await writeFile(join(output, prefix + ".layer-" + layer.layerId + ".commands.json"), JSON.stringify({ layer, commandLog }));
            }
          } catch (error) { row.snapshotError = String(error); }
          finally { if (snapshotId) await cdp.send("LayerTree.releaseSnapshot", { snapshotId }); }
        }
        layers.push(row);
      }
      await writeFile(join(output, prefix + ".layers.json"), JSON.stringify({ layers, observedLayers: [...observed.values()], paints }, null, 2));
    } finally {
      cdp.off("LayerTree.layerTreeDidChange", onTree);
      cdp.off("LayerTree.layerPainted", onPaint);
      await cdp.send("LayerTree.disable");
      await cdp.send("DOM.disable");
    }
  };
}
