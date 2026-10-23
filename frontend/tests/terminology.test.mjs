import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("product copy uses the shared bibliographic and content vocabulary", async () => {
  const locale = JSON.parse(await readFile(new URL("../src/i18n/locales/zh-CN.json", import.meta.url), "utf8"));
  function check(value) {
    if (typeof value === "string") {
      assert.doesNotMatch(value, /元数据|文件版本|书目条目|内容源|书源/);
    } else {
      Object.values(value).forEach(check);
    }
  }
  check(locale);
  assert.equal(locale.book.deleteMetadata, "删除书目");
  assert.equal(locale.book.filesTitle, "书籍内容（{{count}}）");
  assert.equal(locale.plugin.manualIdPlaceholder, "输入书籍 ID");
});
