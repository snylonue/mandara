import { strict as assert } from "node:assert";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import ts from "typescript";

test("API read models distinguish metadata, summaries, and nullable parents", () => {
  const file = fileURLToPath(new URL("api-model-contract.ts", import.meta.url));
  const source = `
    import type { AcquireResult, BookEntry, BookMeta, SeriesBrief, SeriesMeta } from "../src/types";
    declare const book: BookMeta;
    declare const metadata: SeriesMeta;
    const member: BookEntry = { book, files: [], series: metadata };
    const standalone: BookEntry = { book, files: [], series: null };
    const summary: SeriesBrief = { ...metadata, volume_count: 1 };
    const acquisition: AcquireResult = { series: null, books: [standalone] };
    const status = summary.ext.status;
    // @ts-expect-error Metadata is not a summary: it has no member count.
    const invalidSummary: SeriesBrief = metadata;
    // @ts-expect-error Member metadata does not promise a summary count.
    metadata.volume_count;
    // @ts-expect-error Publication status lives only in ext.
    summary.status;
  `;
  const options = {
    strict: true,
    noEmit: true,
    skipLibCheck: true,
    types: [],
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
  };
  const host = ts.createCompilerHost(options);
  const getSourceFile = host.getSourceFile.bind(host);
  host.getSourceFile = (name, languageVersion, onError, shouldCreateNewSourceFile) =>
    name === file ? ts.createSourceFile(name, source, languageVersion, true) :
      getSourceFile(name, languageVersion, onError, shouldCreateNewSourceFile);
  const program = ts.createProgram([file], options, host);
  const errors = ts.getPreEmitDiagnostics(program);
  assert.equal(errors.length, 0, errors.map((error) =>
    ts.flattenDiagnosticMessageText(error.messageText, "\n")).join("\n"));
});
