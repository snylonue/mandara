import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "vite";

test("keeps reader and screenshot code behind their production loading boundaries", async () => {
  const result = await build({
    root: fileURLToPath(new URL("../", import.meta.url)),
    configFile: fileURLToPath(new URL("../vite.config.ts", import.meta.url)),
    logLevel: "silent",
    build: { write: false },
  });
  const chunks = result.output.filter(output => output.type === "chunk");
  const byName = new Map(chunks.map(chunk => [chunk.fileName, chunk]));
  function staticModules(root) {
    const visited = new Set();
    const modules = new Set();
    function visit(chunk) {
      if (!chunk || visited.has(chunk.fileName)) return;
      visited.add(chunk.fileName);
      for (const id of chunk.moduleIds) modules.add(id);
      for (const dependency of chunk.imports) visit(byName.get(dependency));
    }
    visit(root);
    return modules;
  }
  const entry = chunks.find(chunk => chunk.isEntry);
  assert.ok(entry);
  const initial = staticModules(entry);
  assert.ok(![...initial].some(id => /[/]src[/](pages[/]|components[/]Reader[.]tsx$)/.test(id)));
  for (const page of ["ReaderPage", "SharePage"]) {
    const route = chunks.find(chunk => chunk.moduleIds.some(id => id.endsWith("/src/pages/" + page + ".tsx")));
    assert.ok(route?.isDynamicEntry, page + " must remain a lazy route");
    const modules = [...staticModules(route)];
    assert.ok(modules.some(id => id.endsWith("/src/components/Reader.tsx")));
    assert.ok(!modules.some(id => id.endsWith("/src/epubScreenshot.ts")));
  }
});
