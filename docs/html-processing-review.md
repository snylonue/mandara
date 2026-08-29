# HTML processing review: mature library vs hand-rolled parsing, and newtype typing

Scope: every place the codebase turns raw HTML/text into content or edits HTML
strings, and whether switching to a mature HTML library pays off — plus whether a
newtype can make the "this is already-escaped/sanitized HTML" invariant explicit.

## Where HTML is (and is not) parsed today

| Site | Code | Parsing approach |
|---|---|---|
| epub parser | `bookshelf-formats/src/epub.rs` | **mature**: `scraper`/`html5ever` (`Html::parse_document`, `Selector`, `ElementRef`) |
| txt/plugin text → HTML | `bookshelf-formats/src/htmlize.rs` | **hand-rolled emit** (escaping + `<p>`/`<br>`/`<figure>` assembly) |
| plugin chapters, host-side | `bookshelf-server/src/service/library.rs` | calls `htmlize::plugin_text_to_html`, then `annotate_image_dims` |
| image `src` surgery | `library.rs::image_ids_in_html` / `annotate_image_dimensions` | **hand-rolled string scanning** |
| wenku8 guest plugin | `plugins/wenku8-plugin/src/lib.rs` | **hand-rolled scanning** (`content_div`, `strip_contentdp`, `html_to_text`, `decode_entities`, `parse_toc`, …) |
| reader/wiki/bangumi/hello guest plugins | — | JSON API only, no HTML |
| imgdim | `bookshelf-formats/src/imgdim.rs` | binary header sniffing, out of scope |

So the "hand-rolled HTML" surface is three places: `htmlize.rs` (text→HTML
construction), the host-side `annotate_*` string surgery, and the wenku8 guest.

## Would a mature library pay off?

### Native server side (`bookshelf-formats`, `bookshelf-server`) — yes for some sites, no for others

- **epub already uses scraper** (which sits on `html5ever`). The native cost is
  already paid; the wasm target doesn't matter here because the server runs on
  x86_64/aarch64. The remaining hand-rolled pieces are:

  - `htmlize.rs` — **construction**, not parsing. Its input is plain text with a
    tiny convention language (plate-list lines, `[插图NN]` marks). No HTML
    parser would simplify this: escaping text for inclusion and assembling a few
    known tags is exactly what a string writer does best, and what every HTML
    *parser* explicitly refuses to re-emit byte-for-byte. A parser round-trip
    would only add canonicalization risk (attribute reordering, tag
    normalization, self-closing rewriting) with zero correctness gain.

    One nuance: this is *half* wenku8-specific. The **function entry point** is
    a general storage-unification ingest boundary (introduced in `aea4e22`,
    design `docs/storage-unification-design.md` §4.1): `text_to_html` serves
    txt uploads, and `plugin_text_to_html` serves *every* chapter-mode
    plugin's text (`upsert_chapters` / `get_chapter` in `library.rs` apply it
    to any source). But the **convention language it expands is wenku8-shaped
    by origin** — the `[插图] 共 N 张` head, the `N. <url>` plate-list line, and
    the `[插图NN] <url>` inline mark are all copied from wenku8's page layout
    (the design doc itself notes "the `[插图NN] url` contract lives in the
    wenku8 plugin's source code"; `docs/plugins.md` §chapters says the inline
    form is "the form the wenku8 plugin produces" when it resolves a
    print-book `（插图NNN）` mark). And today **only wenku8 emits those
    conventions** — hello/reader/wiki send plain prose, so for them
    `plugin_text_to_html` is just the txt converter.

    So the real state is: a *general* host-side boundary whose *one concrete
    producer* is wenku8. If a second page-scraping plugin wants plates, it
    either speaks the same convention (then the language is generic and
    the doc should say so) or needs its own transformation. That coupling is
    worth calling out but it does not change the parsing conclusion below:
    no HTML parser is needed to expand this tiny line-convention language —
    and the wasm-size argument applies to wenku8's *own* HTML scraping
    (`content_div`/`html_to_text`), which this same review covers.

  - `annotate_image_dims` / `image_ids_in_html` — **surgical string edits** on
    HTML we *ourselves* produced a few lines earlier (the `src="/api/images/{id}"`
    references are emitted by `htmlize`/`ingest_parsed_book`, so their spelling
    is under our control). Rewriting this with a parse→mutate→serialize pass via
    `scraper` would be strictly worse: `scraper`'s tree API cannot modify
    attributes in place (you must build a new tree or re-serialize the `Element`)
    and re-serializing the whole chapter loses the canonical formatting
    `htmlize` produced. The current needle-scan is correct
    *because the needle is a literal we emit*; the failure mode is only if a
    *different* writer emits a non-canonical variant (single quotes, self-closing
    `<img/>`, uppercase `SRC=`).

  So on the native side the realistic upgrade is **not** "use a parser where we
  scan strings" but **"parse in more places that ingest untrusted HTML"** — and
  there is exactly one such place left: plugin `chapter` content. Today plugins
  return *text* and the host runs `plugin_text_to_html`, so untrusted markup
  cannot enter the reader. The design doc already leans toward keeping it that
  way:

  > Q4 Chapter HTML … should `chapter` gain a `format` field so plugins can
  > return HTML and the host sanitizes it through the existing EPUB pipeline?
  > … Leaning: yes in P2, default `text`. — `docs/plugin-http-api-design.md`

  If/when P2 adds `chapter.format = "html"`, the host must **not** accept the
  string as-is: it should run it through the same whitelist sanitizer epub uses
  (`DROP_TAGS`/`ALLOWED_TAGS`/`allowed_attrs` in `epub.rs`). That sanitizer is
  the one piece genuinely worth extracting into a shared function (it already
  lives in `epub.rs` and would move to `htmlize.rs` or a new `sanitize.rs`) so
  both the epub path and the future plugin-HTML path share one implementation.

### wenku8 guest plugin (wasm32-unknown-unknown) — no, and the number is decisive

Measured by building a scratch cdylib for `wasm32-unknown-unknown --release`
with the same codegen settings as the real plugins (`opt-level="s"`):

| Dependency set | wasm32 binary size |
|---|---|
| empty (`#[no_mangle] fn test() -> u32 {42}`) | 0.3 KB |
| + `encoding_rs` (what wenku8 already pulls) | 185 KB |
| + `scraper` 0.27 (`html5ever` 0.39 + `markup5ever` + `selectors` + `cssparser` + `tendril`) | **~820–824 KB** |

The current shipped wenku8 component is 280 KB **including** the entire
hand-rolled parser and all plugin logic. Adding `scraper` would multiply the
plugin's footprint ~3× for the *privilege* of replacing ~14 small pure-text
helpers — a bad trade for a component whose whole job is to be a small,
trusted, admin-deployed extractor for one known site.

It's also the wrong tool: the wenku8 helpers are not "parse HTML" but "recognize
an exact, known vendor layout" (`<td class="vcss">` rows, a `<div id="content">`
with nested `<div class="divimage">`, an ad `<ul id="contentdp">`). `content_div`
is a depth-aware tag scanner because the "content" div nests; `html5ever` would
handle the nesting for free, but it would also *repair* malformed markup (e.g.
an unclosed `<div>` in the page) — silently changing the inner HTML the helper
deliberately returns verbatim. The hand-rolled version's failure modes are
mechanical and reviewable; html5ever's are algorithmic and invisible.

If a wenku8 rewrite ever happens, the right tool is a tiny *tokenizer* (skip
tags, keep text) or a `html-to-text` crate, not a full tree builder — but even
then the size multiplier above argues for keeping the current code until the
site layout actually breaks.

### The real recurring bug class

The hand-rolled code's genuine weakness is not correctness of any single helper
(they are all unit-tested against fixtures) but **the absence of a type-level
distinction between "plain text" and "already-escaped/sanitized HTML"**:

- `ParsedChapter.content` is `String` — a raw `String` could hold unescaped
  user text (XSS), sanitized HTML from epub, or text-converted HTML from
  `htmlize`, and nothing in the type system tells you which.
- `htmlize::text_to_html(&str) -> String` and `plugin_text_to_html(&str) ->
  String` both take `&str` and return `String`: nothing stops a caller from
  passing already-HTML text in (double-escaping → literal `&lt;` in the reader),
  or passing a `figure` line in as plain text (it would get `&lt;`-escaped and
  render as literal text).
- `annotate_image_dimensions(html: &str, …) -> String` takes the *output* of
  the above — the two call sites couple by convention, not by type.

## Newtype pattern: does it help, and how far to take it

The pattern is worth adopting for exactly this distinction, but **scoped to the
host-side ingestion pipeline**, not to every `String` in the codebase. **The
field must be private** — a `pub` newtype is only nominal typing for API
disambiguation; it carries *zero* invariant protection, because any caller can
write `ChapterHtml(unescaped_raw_string)` and bypass the escape/sanitize
constructors entirely. With a private field the only ways to obtain a value are
the gated constructors, and the invariant becomes checked instead of
conventional:

```rust
/// Plain text from a plugin/txt source. Not yet safe to embed in HTML.
pub struct PlainText(String);

/// HTML that has been escaped (text→HTML) or sanitized (epub whitelist).
/// Constructible only via escape/sanitize — a raw `String` cannot be
/// promoted into display-safe HTML without an explicit conversion.
pub struct ChapterHtml(String);

impl PlainText {
    pub fn new(s: String) -> Self { Self(s) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl ChapterHtml {
    /// Text → HTML: escapes and paragraph-wraps (`htmlize::text_to_html`).
    pub fn from_text(t: &PlainText) -> Self { Self(text_to_html(t.as_str())) }
    /// Sanitized markup (epub whitelist / future plugin-HTML).
    pub fn from_sanitized(s: String) -> Self { Self(s) }
    pub fn as_str(&self) -> &str { &self.0 }
}
```

Note `from_sanitized` is *not* a gap: it takes input that has already been
through the whitelist sanitizer (epub's `clean_xhtml` / the shared sanitizer),
and it lives next to that sanitizer in the same module, so the trust boundary
is a constructor call rather than a doc comment. The reader-facing escape
path (`from_text`) is the only one that takes raw text.

The three moves that would actually prevent bugs:

1. **`text_to_html` / `plugin_text_to_html` take `&PlainText`, return
   `ChapterHtml`.** Callers who feed raw `&str` are forced to mark intent
   (`PlainText::new(…)` / `.as_str()`) or call the escape constructor
   explicitly; double-escaping a `ChapterHtml` becomes impossible without an
   explicit `.as_str()` back into `PlainText`-land.

2. **`ParsedChapter.content` becomes `ChapterHtml`.** Then `ingest_parsed_book`,
   the chapter upserts, and `get_chapter` all carry the invariant "what's stored
   is display-safe HTML" from construction to DB write to reader — and the
   DB write itself must call `.as_str()`, which is exactly where a reviewer
   can see and audit the trust boundary.

3. **`annotate_image_dimensions(html: &ChapterHtml, …) -> ChapterHtml`** — the
   signature now says "takes our canonical HTML, returns our canonical HTML",
   and the needle-based scan is no longer reachable with arbitrary caller HTML.

Don't take it further: wrapping every `String` (chapter titles, series names,
URLs, attribute values) in `Title`/`Href`/… newtypes adds ceremony without
catching a real bug — the two invariants that matter here are *text vs HTML*
and *sanitized vs unsanitized*, and both live at the one ingestion boundary.

## Recommendation

1. **Keep** the wenku8 guest hand-rolled: the wasm size cost (~+820 KB, ~3× the
   whole plugin) is not justified by the tiny vendor-layout recognition it does.
   If the layout breaks, revisit with a tokenizer, not a tree builder.
2. **Keep** `htmlize.rs` hand-rolled: it is construction, not parsing. The entry
   point is the general txt/plugin ingest boundary, but its illustration
   conventions are wenku8-shaped and only wenku8 emits them — if a second
   source ever needs plates, generalize the convention in `docs/plugins.md`
   (or add a per-source transform) instead of reaching for an HTML parser.
3. **When** `chapter.format="html"` lands (plugin-http-api-design Q4), **share
   the epub whitelist sanitizer** (extract it from `epub.rs` into a shared
   function) and run plugin HTML through it — that is the one real parser that
   is missing today.
4. **Adopt the newtype pattern** at the ingestion boundary (`PlainText` →
   `ChapterHtml`) **with private fields and gated constructors** as designed
   above; a `pub` newtype would add ceremony without any invariant protection.
   Done that way, the double-escape and raw-string-injection bug classes become
   unrepresentable instead of merely untested.

Net: the mature library pays off on the native side **only** where untrusted
HTML is ingested (future plugin-HTML through the shared sanitizer); on the wasm
side it is a clear loss; the newtype is the cheap, high-value fix that applies
everywhere today.