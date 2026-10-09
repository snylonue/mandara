# Online EPUB fidelity audit

## Scope and interpretation

This audit follows the [fidelity issue report](<docs/epub-rendering-fidelity-issue-report.md>) and the package, spine, navigation, resource and linking rules in [EPUB 3.3](https://www.w3.org/TR/epub-33/). EPUB 2 NCX and legacy package metadata are also covered. The linked issue report describes the pre-audit importer; this document records the classified outcome.

The library on port 8112 contained 17 books and seven EPUB files. All seven originals were downloaded and inspected without replacing production chapters, reading sessions or shares. Downloads, validator reports and rendered publication content remain in ignored local audit artifacts, not version control. Credentials are not included in this report.

**The largest content loss was our importer, not merely bad uploads.** Stored production data had 70 chapter documents and 21 extracted images. The corrected compatible parser preserves 191 primary spine documents plus one auxiliary document and extracts 128 per-file deduplicated image resources across the seven files (copies may share image bytes). Counts describe stored content documents, not a publisher's logical chapter divisions. Covers stored separately are not included in extracted-image counts.

These are implementation and offline verification results, **not a claim that production chapters have been reimported or that a complete EPUB reading system has been certified**. EPUBCheck validates source packaging and markup; it does not establish rendering fidelity.

## File-by-file results

P/T/I means primary documents / total addressable documents / distinct extracted images. Production is the originally observed stored chapter/image count, not a fresh execution of the corrected parser. Both current modes preserve the same reading order.

| File ID prefix | Publication | Production C/I | Current strict P/T/I | Current compatible P/T/I | EPUBCheck error occurrences / JSON groups |
| --- | --- | --- | --- | --- | --- |
| 5152b64f | 異世界転生周回プレイ, copy A | 1/0 | 33/33/23 | 33/33/23 | 2/2 |
| 6c346703 | 異世界転生周回プレイ, copy B | 1/0 | 33/33/25 | 33/33/25 | 2/2 |
| 51f911a5 | 这里是终局停滞委员会。 | 27/18 | 27/28/18 | 27/28/18 | 1/1 |
| 65e11e51 | 新編古典恋心中 | 14/0 | 29/29/0 | 29/29/21 | 74/34 |
| bb83993d | 紫苑的血族 | 14/0 | 19/19/17 | 19/19/17 | 10/3 |
| dd0a2033 | 追放されるは我らにあり! (1) | 11/0 | 45/45/21 | 45/45/21 | 26/4 |
| d5a5b3e2 | 異セカイ系 | 2/3 | 5/5/3 | 5/5/3 | 2/1 |

Validator: EPUBCheck 5.3.0. Every original fails source validation; none has fatal errors or warnings in the saved reports. Its CLI counts error occurrences, while the JSON checker.nError field counts grouped messages and individual location lists may be capped. In particular, the 74 occurrences for 新編 are not 74 distinct problem types; the saved JSON has 34 groups and 58 listed locations. Parser diagnostic counts are a third, independent measure.

### Already present before this audit

The pre-audit code already supported ordinary OPF/spine parsing, EPUB 3 nav, EPUB 2 NCX, basic covers and partial incomplete-navigation recovery introduced by commit 1c9bfd8. Existing navigation hierarchy tests and the long-body recovery case remain useful regressions. That heuristic was not sufficient evidence that every valid spine page survived.

The 18 images in 这里是终局停滞委员会。 and three images in 異セカイ系 were already present in production. Their counts alone do not demonstrate that CSS, footnotes, metadata or all short pages were rendered correctly. Stored production data also cannot prove whether a newer parser fix had been deployed after original ingestion.

## Application defects repaired

| Issue | Corrected behavior and evidence |
| --- | --- |
| EPUB-FID-001 | Preserve every supported linear spine document, including blank, headingless, short and image-only pages. TOC membership is no longer a content filter. Keep supported auxiliary spine and explicitly linked non-spine documents addressable outside default primary order. Persist the linear flag; old metadata and migrated rows default to primary. |
| EPUB-FID-002 | Resolve references against their containing document as URLs; separate canonical ZIP path, query and decoded fragment. Decode once, load the exact Unicode ZIP entry, and never use the dependency's percent-decoding fallback. Reject absolute, active and protocol-relative resource URLs, including the synthetic resolver origin. Tests cover nested directories, encoded names, literal percent signs and internal fragments. |
| EPUB-FID-003 | Select EPUB 3 TOC navigation by namespace and whitespace-separated type tokens. Landmarks and page lists cannot replace the TOC. Preserve nested branches and fragment destinations. |
| EPUB-FID-004 | Select EPUB 2 NCX through the spine's declared TOC identifier rather than arbitrary manifest iteration; retain deterministic constrained malformed-package fallbacks. |
| EPUB-FID-005 | Retain readable SVG and image-only documents and preserve supported passive SVG/MathML content. Follow usable manifest fallback chains for foreign spine media, diagnose missing/cyclic chains and retain original navigation aliases. |
| EPUB-FID-006 | Preserve full passive document structure, publisher CSS, classes, inline styles, ruby, tables, lists, language/direction and supported semantic elements. Resolve CSS imports/images/fonts and SVG assets locally. Render inside a script-free, CSP-restricted iframe so publisher CSS cannot restyle the application. Support vertical scrolling and declared fixed-layout viewport scaling. Native fragment state activates publisher :target rules and reveals collapsed details; SVG links and cross-document footnote/backlink navigation remain functional. |
| EPUB-FID-007 | Preserve ordered author creators, distinguish declared translator/illustrator roles, and trim formatting whitespace around role codes. Keep contributor records and repeated titles, descriptions, languages, identifiers and refinements in extended metadata. Select a deterministic non-empty primary display title. All three Japanese files with two declared author creators now retain both. |
| EPUB-FID-008 | Separate exact container access, URI resolution, resource-graph/spine materialization and passive rendering policy. Expose document/reference/reason diagnostics for missing resources, blocked active behavior, unsupported presentation and producer recovery. Auditing can compare strict references with compatible output without writing a database. |

The [API contract](<docs/api/openapi.yaml>) describes full reader documents, internal chapter links and optional linear metadata; the frontend types were regenerated from it. Reader previous/next traversal and its primary-content progress denominator exclude auxiliary documents. File chapter counts still count all addressable stored documents.

## Source defects and degradation

### 新編古典恋心中

There are two source problem classes: incorrect relative references and duplicate physical resources in the manifest. The navigation document is one directory deeper than its references assume. The same mistake affects illustrations and publisher CSS.

    Source document:   OEBPS/text/nav.xhtml
    Authored href:     text/ch_00_prologue.xhtml
    Standard target:   OEBPS/text/text/ch_00_prologue.xhtml (absent)
    Recovered target:  OEBPS/text/ch_00_prologue.xhtml (declared and present)

    Illustration href: images/image_rsrcJVN.jpg
    Standard target:   OEBPS/text/images/image_rsrcJVN.jpg (absent)
    Recovered target:  OEBPS/images/image_rsrcJVN.jpg (declared and present)

    Stylesheet href:   style.css
    Standard target:   OEBPS/text/style.css (absent)
    Recovered target:  OEBPS/style.css (declared and present)

Strict URL interpretation correctly finds no illustrations; that is a source-reference fault. Dropping 15 otherwise readable spine pages was our separate content-selection defect and is now fixed even in strict mode.

Compatible output retains all 29 primary documents, loads 21 distinct illustrations and restores the ten top-level authored TOC branches instead of the 29-entry synthesized strict TOC. It records 60 unique package-relative recoveries and two duplicate-manifest diagnostics. Duplicate same-media-type declarations are coalesced by physical path; conflicting media types are not silently selected. The original source remains invalid after compatibility rendering.

### 異世界転生周回プレイ, both copies

All eight declared publisher CSS resources in each copy are zero-byte. Empty CSS is legal syntax; these files provide no usable publisher typography/layout rules. The surviving class conventions indicate likely styling degradation, not a CSS syntax violation. The root vrtl class is a publisher convention, not EPUB semantics. Each copy also has invalid heading nesting and a broken navigation stylesheet reference escaping the intended package directory (RSC-005 and RSC-007).

Correct URI loading and spine preservation restore 33 documents and 23/25 distinct illustrations. The two byte-distinct originals must not be treated as interchangeable. Their missing CSS cannot be reconstructed faithfully.

### 追放されるは我らにあり! (1)

All nine declared CSS resources are zero-byte. Source validation additionally reports four fixed-layout viewport omissions (HTM-046), invalid heading/division nesting and a broken navigation stylesheet reference. The corrected importer retains 45 documents and 21 distinct illustrations instead of 11 documents with none.

Valid declared viewport dimensions are respected; absent publisher dimensions, original fonts, margins and page composition are not fabricated as standards facts.

### 这里是终局停滞委员会。

The actual TOC resolves. A landmarks entry references an absent TOC document (RSC-007); that does not justify throwing away the real reading order. All 27 primary documents and 18 illustrations were already stored, but the auxiliary navigation document is now also addressable without joining previous/next traversal.

### 紫苑的血族

EPUBCheck reports duplicate IDs (three grouped RSC-005 messages, ten occurrences). Automatically renaming them would change CSS and fragment meaning without evidence of the intended target, so no such rewrite is performed. Passive browser markup recovery retains readable text. Six event-handler attributes are stripped by the reader's security policy; this is separate from duplicate-ID validation errors.

A generic XML library's inability to expand XHTML DTD entities such as non-breaking spaces is not, by itself, evidence of an invalid publication. The corrected importer retains 19 documents and 17 distinct illustrations.

### 異セカイ系

There are two EPUB 2 markup errors for unsupported role attributes, grouped into one RSC-005 message. The NCX primarily indexes notes/back references rather than logical prose chapters. Poor navigation is not automatically a normative TOC violation, and it must never determine which spine text is retained. The current parser retains five primary documents and all three illustrations.

## Bounded compatibility, not source repair

1. Standard resolution wins whenever its exact declared target exists.
2. Otherwise, compatible mode tries a bounded set of package-relative interpretations, including a limited removal of producer-leading parent/dot components. It accepts only one unique existing manifested ZIP target. There is no basename search, arbitrary ZIP search, network fetch or external/active URL recovery. Ambiguous candidates are diagnosed and not substituted. Fragments and queries remain distinct from resource paths.
3. A low-specificity vertical-direction rule is inferred only for a reflowable vrtl document when every declared publisher stylesheet exists and is exactly zero-byte and there is no meaningful inline stylesheet/style attribute. Non-empty publisher CSS, inline styles, missing CSS and fixed-layout documents suppress the inference. This records compatibility-empty-css-vrtl; it is not a standard interpretation of the class.

Vertical inference applies to 33 documents in copy A, 18 in copy B and 20 in 追放. The numbers differ because document classes and inline/fixed-layout conditions differ. Empty-stylesheet diagnostics remain present. Direction recovery does not recover the publisher's lost fonts, spacing, pagination or ornamentation.

The CLI's --strict flag disables these producer-reference and empty-CSS direction recoveries. It is not an EPUB validator: tolerant markup/navigation recovery and the safe reader policy still apply. Source validation remains a separate EPUBCheck operation.

## Verification and limits

- Workspace cargo check, clippy with all targets and warnings denied, and tests; frontend typecheck and production build.
- All 47 format tests pass; regressions cover spine retention, nav namespaces/tokens, selected NCX, encoded/literal-percent references, exact Unicode entry access, fallback chains, role whitespace, repeated metadata, CSS/SVG/MathML/passive semantics and strict-versus-compatible recovery.
- The [complete-spine text check](<scripts/check-epub-corpus.py>) compares all 192 spine documents with original containers in both export modes. It compares non-whitespace body text after entity decoding, excluding head/style/script text; it is not a preformatted-whitespace or pixel-equivalence test.
- The [offline browser regression](<scripts/test-epub-reader.mjs>) intercepts every request. Synthetic tests verify CSS isolation, blocked scripting/network, ruby/tables, native hidden fragment targets, collapsed auxiliary footnotes/backlinks, encoded fragments, primary-only next order, vertical progress and fixed viewport scaling. Every corpus document is opened; all 154 body-image occurrences load, covering 128 per-file deduplicated extracted image resources. Recovered direction is checked through computed writing mode.
- These are structural, retained-text, resource-loading and behavioral checks, not an exhaustive reference-reader screenshot comparison. Declared valid publisher styling is preserved within the supported passive profile, but browser/font availability can still affect appearance.
- Scripting, forms and remote resources are intentionally blocked. Audio/video playback and responsive-image sets are outside the passive profile and diagnosed; encrypted/obfuscated fonts, all SVG/MathML edge features, multi-page spreads and exact publisher pagination are not fully supported. Missing source CSS or viewport data cannot be reliably reconstructed.
- Diagnostics are exposed by the parser/audit tool; this work does not add a persisted per-book web warning screen. Unsupported reader capabilities are not classified as source validation errors merely because they are unsupported.

## Production safety boundary

A parser fix does not retroactively replace stored chapter HTML. No production reimport or service restart is claimed. The offline browser uses intercepted responses; generated frontend artifacts alone do not prove which backend version port 8112 is running.

Before any approved rollout/reparse, back up the database and original resources, preview the new document/TOC mapping and review reading-session/share destinations. Restored frontmatter and short pages shift old numeric chapter indices; an in-place overwrite without source-resource/fragment mapping can move a bookmark to another page. Legacy rows default to primary rather than being guessed from old truncated content. Keep deployment and data migration separate, explicitly authorized operations.

## Original fingerprints

| Full file ID | Downloaded SHA-256 |
| --- | --- |
| 5152b64f0dc140f6a6333f877d69b47b | 8c7d6fcffe6440caa482d220618a9f4f5f53a61f204ac221fb29d6c75ccc35e7 |
| 6c3467036e684a99974e53b229d7e37a | 143f84b520dc29b39b8480457639c70bc6bf6bfd0e655a050598617c2b3e44a8 |
| 51f911a5fa744792807c0e41e9c05f2a | 13ef30934cc8b535c0ccd17893ea570a36c55b28ae53c0cfa5e6b1825eb56cd8 |
| 65e11e51f1b04fff8dc80da985dec43a | f0a29e1790b1841b4431ff97c09f15a6d20ef998066445324c7c148361a8c8bc |
| bb83993d8c6c404fb67c0b2eb59a38b2 | 7dca91cb3db61ba88a91912ff25e74894a41f3e7eba74aa57dc89af3063462d4 |
| dd0a203388274be99e9cd67efcbd2d14 | 6cfce1b9e7de54355a261cb64f425cff795548b64ea5769f6813c125ae19777e |
| d5a5b3e230d54a1593d74f5db1554e4f | 5f5431d81889a73c1412d831c24d2e7305711ef9d53e359e2c31725a531974c6 |

## Reproduction

Use the [read-only audit example](<crates/mandara-formats/examples/epub_audit.rs>) with authenticated downloads kept outside version control. Example commands for the malformed-reference publication:

    cargo run -p mandara-formats --example epub_audit -- --strict target/epub-audit/65e11e51f1b04fff8dc80da985dec43a.epub
    MANDARA_EPUB_AUDIT_OUTPUT_DIR=target/epub-audit/rendered cargo run -p mandara-formats --example epub_audit -- target/epub-audit/65e11e51f1b04fff8dc80da985dec43a.epub
    nix run nixpkgs#epubcheck -- target/epub-audit/65e11e51f1b04fff8dc80da985dec43a.epub
    MANDARA_EPUB_AUDIT_OUTPUT_DIR=target/epub-audit/rendered cargo run -p mandara-formats --example epub_audit -- target/epub-audit/*.epub
    python3 scripts/check-epub-corpus.py target/epub-audit/rendered
    PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs CHROMIUM_BIN=/absolute/path/to/chromium node scripts/test-epub-reader.mjs target/epub-audit/rendered/*.json

EPUBCheck exits 1 for these invalid originals; that is expected source-validation evidence, not a failed application regression. The browser command takes export paths and requires Playwright plus a Chromium executable as documented in the linked harness.
