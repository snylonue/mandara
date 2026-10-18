import { strict as assert } from "node:assert";
import { test } from "node:test";
import { shelfBookTitle } from "../src/shelfTitle.ts";

const entry = (title, seriesTitle = null) => ({
  book: { title },
  series: seriesTitle === null ? null : { title: seriesTitle },
});

test("a lone volume keeps its series name and ordinal", () => {
  assert.equal(shelfBookTitle(entry("第一卷", "在那开满花的山丘，我想见到你。")),
    "在那开满花的山丘，我想见到你。 · 第一卷");
});

test("does not change meaningful volume titles or aliases", () => {
  assert.equal(shelfBookTitle(entry("第一卷 明神凛音绝不出错", "你的解谜由我证明(你的解谜由我作答)")),
    "第一卷 明神凛音绝不出错");
  assert.equal(shelfBookTitle(entry("運命の人は、嫁の妹でした。", "命定之人是妻子的妹妹。")),
    "運命の人は、嫁の妹でした。");
});

test("recognizes numeric and full-width ordinal-only labels", () => {
  for (const title of ["第1卷", "第２卷", "第十二卷", "第 3 卷"]) {
    assert.equal(shelfBookTitle(entry(title, "系列书名")), "系列书名 · " + title);
  }
});

test("does not duplicate an already qualified title", () => {
  assert.equal(shelfBookTitle(entry("示例小说 第1卷", "示例小说")), "示例小说 第1卷");
  assert.equal(shelfBookTitle(entry("示例小说", "示例小说")), "示例小说");
});

test("does not guess a series name for standalone titles", () => {
  for (const title of ["第一卷", "异世界系 異セカイ系link world", "你的解谜由我证明(你的解谜由我作答)"]) {
    assert.equal(shelfBookTitle(entry(title)), title);
  }
});

