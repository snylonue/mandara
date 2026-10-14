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
| `access-token` | Optional existing login-session Bearer token for restricted catalog metadata. Never sent to illustration hosts. |

General-audience reads do not require an account according to the upstream routes. Tokens are ordinary instance configuration: protect admin access and database backups. No password/login automation or token refresh is provided. Unauthorized responses are not retried anonymously.

## Identity and loading

- Web books/content: `web:<providerId>:<novelId>`, e.g. `web:syosetu:n1234ab`. Metadata prefers translated titles/descriptions and preserves the original title. TOC headings without chapter IDs are omitted. Each chapter call reloads the TOC to resolve its index. Prose is escaped HTML; `<图片>URL` paragraphs become stored-image figures (failed image downloads retain the HTTP(S) URL).
- Wenku series: `wenku:<novelId>`.
- Wenku books/content: `wenku-volume:<novelId>:<encoded-upload-filename>`. UTF-8 percent encoding is applied to each ID component. Filenames, not list positions, identify actual `volumeJp` uploads. The API has no reliable ASIN-to-upload association: file titles come from filenames, with series-level authors/covers/illustrators. Admin-only Chinese uploads (`volumeZh`) are not exposed.

Web automatic mode chooses the first complete paragraph array; absent/malformed translations fall back to Japanese. A fixed translator with no complete array returns unavailable, never a missing-translation banner. Original illustration markers take precedence. Empty chapters remain valid. After changing translation settings, rematerialize content to replace cached bodies.

Wenku requires a positive chapter count for at least one selected translator. Downloads use `mode=zh&translationsMode=priority` and repeated `translations` parameters. Japanese-only downloads are unsupported. **Partial translations remain partial**: the upstream exporter retains original EPUB chapters or inserts missing-translation notes in TXT segments without translations. The plugin does not start translation jobs. The host follows download redirects and parses the returned files. Size/time caps still apply; large volumes may require increasing the host fetch size cap.

Search uses pages of 100, slicing across page boundaries for arbitrary host offsets. Upstream supplies page counts, so totals are estimates until the last page is fetched. HTTP/JSON/transport failures are logged and return unavailable/empty results. Transport/timeouts and 5xx get at most one retry; 401/403/404/429 are final.

## Verification

```sh
cargo test -p novelia-plugin
./scripts/build-plugins.sh novelia
cargo test -p mandara-plugin --test novelia -- --ignored
```

The ignored integration test requires the built component and exercises actual WIT configuration, HTTP imports, image storage and download redirects against local mock endpoints. It needs neither an account nor external network.

The API contract was inspected at upstream revision [cbfd3f8](https://github.com/auto-novel/auto-novel/tree/cbfd3f8bf754d848c7401812b3922ad3677cc355): [web API](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/web/src/api/WebNovelApi.ts), [Wenku API](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/web/src/api/WenkuNovelApi.ts), [web routes](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/server/src/main/kotlin/api/RouteWebNovel.kt), [Wenku routes](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/server/src/main/kotlin/api/RouteWenkuNovel.kt).

Live-site verification is separate from these mocks; the implementation environment could not establish TLS to the site.
