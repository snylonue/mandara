# EPUB rendering fidelity issue report

## Objective

The EPUB importer should preserve the original publication's reading experience as closely as the web reader can safely support. The EPUB package is the source of truth for reading order, document membership, navigation, resources, and metadata:

- the OPF spine defines the default reading order;
- the manifest defines publication resources and their media types;
- the EPUB 3 navigation document or EPUB 2 NCX defines navigation structure;
- relative references resolve against the containing document's URL;
- XHTML, SVG, images, CSS, and other supported resources together determine the rendered result.

The importer may remove active or unsafe behavior such as scripts, forms, and external tracking, but it should preserve visible text, document structure, images, internal navigation, CSS-driven layout, and all linear spine content whenever the web reader supports the relevant feature.

This report covers the current implementation in crates/mandara-formats/src/epub.rs and crates/mandara-formats/src/htmlize.rs.

## Executive assessment

The current implementation is a good text-oriented EPUB extractor, but it is not yet a high-fidelity EPUB importer. It correctly handles the common OPF/spine plus EPUB 3 nav or EPUB 2 NCX structure, but it applies chapter heuristics and a restrictive HTML sanitizer that can remove valid publication content. It also does not resolve EPUB URL references completely.

The highest-risk data-loss problems are:

1. valid linear spine documents are discarded when they have no TOC entry or heading;
2. ../, query, and fragment references are not resolved as URI references;
3. SVG content and image-only documents are discarded;
4. CSS and many visible HTML semantics are removed;
5. a valid EPUB 3 navigation document can be misidentified when epub:type contains multiple tokens.

The relevant EPUB 3 specification is [EPUB 3.3](https://www.w3.org/TR/epub-33/), especially its package/spine, navigation document, content document, resource, and linking requirements.

## Issues

### EPUB-FID-001: Preserve every readable linear spine document

**Priority:** P0 for fidelity

**Location:** crates/mandara-formats/src/epub.rs:79-165

**Current behavior:**

The importer starts with the spine, but later keeps a document only when it appears in the TOC, has a heading, or passes a special text-length fallback. It also drops every document whose sanitized result has no text.

**Expected behavior:**

Every manifest resource referenced by a linear="yes" spine item should remain in the reading order, subject only to supported media type and an explicit unsupported-resource policy. A document should not need a TOC entry or heading to be part of the publication.

The EPUB package already provides the standard mechanism for auxiliary spine entries: linear="no". The importer should not infer that a linear="yes" document is auxiliary merely because it lacks a heading or TOC entry.

**Examples currently lost:**

- copyright and title pages;
- prefaces containing paragraphs but no heading;
- short prose chapters;
- image-only illustration or comic pages;
- documents referenced by a TOC entry that contains only a fragment;
- SVG content documents.

**Acceptance criteria:**

- A linear="yes" XHTML document with no TOC entry and no heading remains in chapters.
- A linear="yes" image-only or SVG document is preserved when the reader supports its media type.
- A linear="no" document remains excluded from the default chapter sequence unless explicitly exposed as auxiliary content.
- The order of retained documents exactly follows the OPF spine order.
- Any intentional text-only mode is an explicit import option or product policy, rather than an implicit EPUB parsing rule.

### EPUB-FID-002: Resolve EPUB references as URI references

**Priority:** P0 for fidelity

**Location:** crates/mandara-formats/src/epub.rs:350-355, :447, :501-511, and :634-645

**Current behavior:**

Navigation and image references are joined with Path::join. The path key keeps .. components, and query strings and fragments are not removed before resource lookup. Fragment handling exists only in part of the TOC remapping code.

**Expected behavior:**

All manifest, navigation, NCX, XHTML, image, stylesheet, and internal-link references should follow EPUB URL semantics:

1. resolve the reference against the containing document's directory;
2. separate the query and fragment from the container path;
3. percent-decode the path where required;
4. remove . and .. path segments;
5. look up the canonical container path;
6. retain the fragment separately for internal navigation.

**Examples currently at risk:**

- ../Text/chapter.xhtml from a nested navigation document;
- ../Images/cover.jpg from XHTML in a subdirectory;
- chapter.xhtml?reader=1;
- chapter.xhtml#section-2;
- percent-encoded names such as chapter%20one.xhtml;
- image references with fragments or query strings.

**Acceptance criteria:**

- A fixture with nested directories and ../ references produces the same chapter and image mapping as a browser.
- A TOC href with a fragment maps to the containing chapter and stores the decoded fragment separately.
- Query strings do not become part of the ZIP entry path.
- Percent-encoded paths resolve to the manifest and ZIP entry with the corresponding decoded name.
- The same resolver is used for nav, NCX, XHTML images, stylesheets, and internal links.

### EPUB-FID-003: Match epub:type as a token list

**Priority:** P1

**Location:** crates/mandara-formats/src/epub.rs:299-315

**Current behavior:**

The EPUB 3 TOC selector matches only the exact CSS value epub:type="toc". When that fails, the code selects the first nav element, which can be a landmarks or page-list navigation.

**Expected behavior:**

epub:type is a space-separated list of semantic tokens. The importer should select the navigation element whose token list contains toc, regardless of token order or additional values.

**Acceptance criteria:**

The following all select the TOC navigation:

    <nav epub:type="toc">
    <nav epub:type="toc landmarks">
    <nav epub:type="landmarks toc">

A landmarks or page-list navigation must never be used as the TOC solely because it appears first in the document.

### EPUB-FID-004: Select EPUB 2 NCX through spine@toc

**Priority:** P1

**Location:** crates/mandara-formats/src/epub.rs:280-297

**Current behavior:**

The importer selects the first manifest resource with media type application/x-dtbncx+xml. The manifest is stored in a HashMap, so multiple NCX resources can result in the wrong or non-deterministic NCX being selected.

**Expected behavior:**

For EPUB 2, read the NCX manifest ID from spine@toc, then resolve that manifest item. Only use a MIME-type search as a recovery path when the OPF is incomplete or malformed.

**Acceptance criteria:**

- A fixture containing two NCX resources uses the one named by spine@toc.
- A malformed or missing spine@toc produces a deterministic fallback.
- NCX order follows playOrder where present and document order otherwise.

### EPUB-FID-005: Preserve SVG and image-only content

**Priority:** P1

**Location:** crates/mandara-formats/src/epub.rs:35-36, :101-103, :115-116, :647-652; crates/mandara-formats/src/htmlize.rs:29-33

**Current behavior:**

Only application/xhtml+xml and text/html spine items are accepted. SVG spine documents are skipped. Image-only pages are discarded because the sanitized content has no readable text. Embedded SVG images are explicitly rejected, and SVG, MathML, and Canvas subtrees are dropped by the sanitizer.

**Expected behavior:**

The importer should preserve EPUB 3 SVG content documents and image-only pages when the web reader can render them. If a format cannot yet be displayed, the importer should retain it as a resource or provide a controlled fallback rather than silently deleting it.

**Acceptance criteria:**

- SVG content documents appear in spine order.
- Raster images, SVG images, and image-only XHTML pages remain visible.
- A comic or illustration EPUB does not fail with epub has no readable chapters.
- Unsupported active content is reported separately from missing content.

### EPUB-FID-006: Keep CSS and visible structural semantics

**Priority:** P1 for visual fidelity

**Location:** crates/mandara-formats/src/htmlize.rs:29-99, :152-195

**Current behavior:**

The sanitizer drops stylesheets and inline style attributes, removes SVG, MathML, and media elements, and keeps only a small HTML whitelist. Cross-document links are removed. This produces safe canonical HTML but can substantially change the original layout and visible content.

**Expected behavior:**

The importer should preserve the publication's visual result as far as the reader supports it. Active behavior can be blocked by policy, but passive presentation resources should be retained and scoped to the imported book or chapter.

A safe implementation could:

- rewrite manifest CSS URLs to book-local resource URLs;
- sanitize CSS separately instead of deleting it wholesale;
- keep safe structural tags and attributes needed for layout;
- rewrite internal cross-document links to reader routes;
- keep external links only under the reader's explicit link policy;
- preserve lang, dir, class, role, and accessibility attributes where safe.

**Acceptance criteria:**

- Paragraph indentation, vertical spacing, ruby, tables, figures, writing direction, and fixed-layout metadata remain visually comparable to the source.
- A stylesheet referenced by an XHTML document is loaded from the EPUB resource store after URL rewriting.
- Internal links navigate to the matching chapter and fragment.
- Scripts, forms, event handlers, and unsafe URL schemes remain blocked.
- Sanitization failures do not silently remove the entire visible document.

### EPUB-FID-007: Preserve complete repeated metadata

**Priority:** P2

**Location:** crates/mandara-formats/src/epub.rs:54-61

**Current behavior:**

Only the first dc:creator is stored in authors, and only the first title and description are read through the current convenience methods.

**Expected behavior:**

Repeated Dublin Core metadata should be retained or deliberately resolved according to EPUB metadata semantics. At minimum, all authors should be preserved, with refinements used to distinguish translators and other contributors.

**Acceptance criteria:**

- Multiple dc:creator entries remain available to the metadata model.
- Creator roles do not cause translators to replace authors.
- The primary title is selected deterministically while alternate titles are not silently discarded if the model can store them.

### EPUB-FID-008: Separate safe rendering policy from EPUB parsing

**Priority:** P2

**Location:** crates/mandara-formats/src/epub.rs and crates/mandara-formats/src/htmlize.rs

**Current behavior:**

Parsing, content selection, security sanitization, and text-oriented chapter extraction are coupled. This makes it difficult to tell whether missing content was absent from the EPUB or intentionally removed by the reader policy.

**Expected behavior:**

Use separate stages:

1. parse the OCF container and OPF package;
2. resolve the manifest, spine, navigation, and resource graph;
3. materialize an intermediate representation that preserves source semantics;
4. apply the reader's safe-rendering policy;
5. render or store the resulting content.

The intermediate representation should retain source media type, canonical resource path, query, fragment, and relationships even when a later rendering policy removes or replaces something.

**Acceptance criteria:**

- Diagnostics identify skipped resources and the reason: unsupported media type, unsafe active content, broken reference, or policy filtering.
- A user can distinguish a genuinely missing EPUB resource from a resource intentionally blocked by the reader.
- Tests cover parsing and rendering policy independently.

## Recommended implementation order

1. Introduce a shared EPUB URI/reference resolver and use it for all resource, nav, NCX, image, CSS, and internal-link lookups.
2. Preserve every linear="yes" spine item and remove the heading/TOC heuristics from the core parser.
3. Fix EPUB 3 nav token matching and EPUB 2 spine@toc selection.
4. Add SVG and image-only fixtures and decide how the web reader will render them.
5. Replace unconditional CSS removal with book-scoped CSS storage and sanitization.
6. Rewrite internal links to chapter routes and retain fragments.
7. Expand metadata extraction and add diagnostics for intentional filtering.

## Required regression fixtures

The test suite should include at least these EPUBs or generated fixtures:

- EPUB 3 with nav epub:type="toc landmarks";
- EPUB 3 with TOC, landmarks, and page-list navs in a non-standard order;
- navigation and images referenced through nested ../ paths;
- TOC hrefs containing query strings and fragments;
- percent-encoded filenames;
- EPUB 2 with two NCX files and an explicit spine@toc;
- linear spine documents without TOC entries or headings;
- image-only XHTML spine documents;
- SVG spine documents and SVG image resources;
- a stylesheet using relative image and font references;
- multiple authors and refined creator roles;
- internal cross-document links with fragments.

## Definition of done

The importer is ready for a high-fidelity release when a reference EPUB corpus can be opened in the web reader with the following properties:

- all linear spine documents appear in the correct order;
- TOC hierarchy and fragment targets are preserved;
- all supported images, SVG, stylesheets, and passive resources resolve correctly;
- visible text and structure are not removed by chapter heuristics;
- internal links and reading navigation work;
- unsafe active behavior is blocked with an explicit diagnostic;
- the stored result is visually comparable to the source EPUB on representative text, image-heavy, RTL, ruby, table, and fixed-layout samples.
