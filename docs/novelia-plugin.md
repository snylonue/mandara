# Novelia source plugin

The `novelia-plugin` guest adapts [Novelia](https://n.novelia.cc)'s public JSON API. It supports web-novel search, metadata and lazy chapter loading, plus Wenku series search and independently addressable epub/txt volume downloads.

## Build and register

```sh
./scripts/build-plugins.sh novelia
cp plugins-built/novelia.wasm data/plugins/
```

Restart the server, then register `novelia.wasm` in the admin UI. The plugin build script and Nix package discovery include it automatically (`mandara-plugin-novelia`). Register two instances with different catalogs to expose both web and Wenku searches. All lookups work in either instance; `catalog` selects only the search endpoint.

| Configuration | Meaning |
| --- | --- |
| `base-url` | Site root, default `https://n.novelia.cc`; omit `/api`. Supports compatible mirrors and mocks. |
| `catalog` | `0`: web novels (default); `1`: Wenku series. |
| `providers` | Web-search source IDs: syosetu, kakuyomu, hameln, pixiv, alphapolis, novelup. An empty list searches nothing. |
| `translation` | `0`: GPT → Sakura → Youdao → Japanese; `1`: GPT only; `2`: Sakura only; `3`: Youdao only; `4`: Japanese only (web novels). |
| `access-token` | Existing login-session Bearer token. Required by live web-novel search and restricted catalogs; never sent to illustration hosts. |
| `volume-files` | Optional JSON array of `{ "novel-id": "series ID", "asin": "ASIN", "filename": "upload.epub" }` bindings for files without a unique title match. Default `[]`. |

Live Wenku search, metadata and translated downloads, plus web-novel details and chapters, were verified without an account. Live web-novel search returns HTTP 401 for guests and requires a token; optional authentication in the source routes does not imply anonymous access in the deployed service. Tokens are ordinary instance configuration: protect admin access and database backups. No password/login automation or token refresh is provided. Unauthorized responses are not retried anonymously.

## Identity and loading

- Web books/content: `web:<providerId>:<novelId>`, e.g. `web:syosetu:n1234ab`. Metadata prefers translated titles/descriptions and preserves the original title. TOC headings without chapter IDs are omitted. Each chapter call reloads the TOC to resolve its index. Prose is escaped HTML; `<图片>URL` paragraphs become stored-image figures (failed image downloads retain the HTTP(S) URL).
- Wenku series: `wenku:<novelId>`.
- Wenku books/content: `wenku-asin:<novelId>:<ASIN>`, e.g. `wenku-asin:6806f70f2d549f092c9ae862:B00R4JX10Q`. Books are enumerated from bibliographic `volumes`, not uploads. IDs, titles, per-volume covers and publishers come from metadata and survive upload renames, removals and reordering. Missing/invalid/duplicate ASINs are not replaced with filenames, hashes or list positions. Metadata may exist without content. Admin-only Chinese uploads (`volumeZh`) are not exposed.

The API still takes filenames as download **locators**, but exposes no explicit ASIN-to-upload association. Automatic resolution requires an exact, unique normalized title match in both directions. It applies Unicode NFKC and whitespace normalization, and removes only known publisher/imprint and author suffixes; it never matches by ordinal, substring or fuzzy similarity. Ambiguous or unmatched downloads return unavailable. Use `volume-files` to bind the known ASIN to a specific upload when necessary; stale, duplicate or malformed bindings fail closed. Updating the locator does not change the ASIN identity.

Legacy `wenku-volume:...` filename IDs are intentionally rejected, not silently reinterpreted. Previously imported Wenku content must be reacquired under its ASIN ID; existing cached bodies are not migrated or deleted.

Web automatic mode chooses the first complete paragraph array; absent/malformed translations fall back to Japanese. A fixed translator with no complete array returns unavailable, never a missing-translation banner. Original illustration markers take precedence. Empty chapters remain valid. After changing translation settings, rematerialize content to replace cached bodies.

Wenku requires a positive chapter count for at least one selected translator. Downloads use `mode=zh&translationsMode=priority` and repeated `translations` parameters. Japanese-only downloads are unsupported. **Partial translations remain partial**: the upstream exporter retains original EPUB chapters or inserts missing-translation notes in TXT segments without translations. The plugin does not start translation jobs. The host follows download redirects and parses the returned files. Size/time caps still apply; large volumes may require increasing the host fetch size cap.

Search uses pages of 100, slicing across page boundaries for arbitrary host offsets. Upstream supplies page counts, so totals are estimates until the last page is fetched. HTTP/JSON/transport failures are logged and return unavailable/empty results. Transport/timeouts and 5xx get at most one retry; 401/403/404/429 are final.

## Verification

```sh
cargo test -p novelia-plugin
./scripts/build-plugins.sh novelia
cargo test -p mandara-plugin --test novelia compiled_guest -- --ignored
# Opt-in network validation (downloads one translated EPUB):
cargo test -p mandara-plugin --test novelia live_guest -- --ignored
```

The ignored mock integration test requires the built component and exercises actual WIT configuration, ASIN lookups, HTTP imports, image storage and download redirects against local endpoints. It needs neither an account nor external network. The separate live test validates anonymous Wenku search, ASIN metadata/downloads and web metadata/chapters. Set `NOVELIA_TEST_ACCESS_TOKEN` to also validate authenticated web search; the token is not persisted. A captured live Wenku catalog fixture tests every edition against unordered, decorated upload filenames.

The API contract was inspected at upstream revision [cbfd3f8](https://github.com/auto-novel/auto-novel/tree/cbfd3f8bf754d848c7401812b3922ad3677cc355): [web API](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/web/src/api/WebNovelApi.ts), [Wenku API](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/web/src/api/WenkuNovelApi.ts), [web routes](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/server/src/main/kotlin/api/RouteWebNovel.kt), [Wenku routes](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/server/src/main/kotlin/api/RouteWenkuNovel.kt).

Live catalog identity was checked against [Wenku metadata](https://n.novelia.cc/api/wenku/6806f70f2d549f092c9ae862); anonymous web lookup and chapters were checked with `web:syosetu:n9669bk`. Live tests remain opt-in because network availability, permissions and upstream catalog data can change.
