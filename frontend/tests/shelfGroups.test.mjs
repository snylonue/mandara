import { strict as assert } from "node:assert";
import { test } from "node:test";
import { groupBySeries } from "../src/shelfGroups.ts";

const series = { id: "series-1", title: "在那开满花的山丘，我想见到你。" };
const entry = (id, title, parent = series) => ({ book: { id, title }, series: parent, files: [], progress: null });

test("a single visible volume is a series card using only the series name", () => {
  const volume = entry("book-1", "第一卷");
  assert.deepEqual(groupBySeries([volume]), [{ kind: "series", series, volumes: [volume] }]);
});

test("groups multiple volumes under the same unchanged series title", () => {
  const volumes = [entry("book-1", "第一卷"), entry("book-2", "第二卷")];
  assert.deepEqual(groupBySeries(volumes), [{ kind: "series", series, volumes }]);
});

test("standalone books retain their own titles and positions", () => {
  const standalone = entry("standalone", "异世界系 異セカイ系link world", null);
  const volume = entry("book-1", "第一卷");
  assert.deepEqual(groupBySeries([standalone, volume]), [
    { kind: "book", entry: standalone }, { kind: "series", series, volumes: [volume] },
  ]);
});

test("does not merge distinct series sharing a title or change source data", () => {
  const volumes = [entry("book-1", "第一卷"), entry("book-2", "第一卷", { ...series, id: "series-2" })];
  const before = structuredClone(volumes);
  assert.equal(groupBySeries(volumes).length, 2);
  assert.deepEqual(volumes, before);
  assert.deepEqual(groupBySeries([]), []);
});
