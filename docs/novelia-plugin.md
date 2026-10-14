# Novelia source plugin

The guest adapts [Novelia](https://n.novelia.cc): web novels provide searchable metadata and lazy chapters; Wenku provides **content only**, through EPUB/TXT downloads. Wenku never appears in metadata search, book lookup or series expansion.

## Build and register

```sh
./scripts/build-plugins.sh novelia
cp plugins-built/novelia.wasm data/plugins/
```

Restart the server, then register the component in the admin UI. Nix package discovery includes it as `mandara-plugin-novelia`. One instance can provide web metadata/chapters and Wenku content. There is no `catalog` setting: searches are exclusively web novels. To use Wenku, create/select book metadata elsewhere and attach this instance as its content source with a Wenku content ID.

## Configuration and login

| Configuration | Meaning |
| --- | --- |
| `base-url` | Site root, default `https://n.novelia.cc`; omit `/api`. |
| `providers` | Web-search sources, all enabled by default: syosetu, kakuyomu, hameln, pixiv, alphapolis, novelup. An empty list searches nothing. |
| `translation` | `0`: GPT → Sakura → Youdao → Japanese; `1`: GPT only; `2`: Sakura only; `3`: Youdao only; `4`: Japanese only (web novels). |
| `username` | Account email or username, default empty. |
| `password` | Account password, default empty. Leading/trailing whitespace is preserved. |
| `auth-url` | Authentication service root, default `https://auth.novelia.cc`; omit `/api`. HTTPS is required except for loopback mock servers. |

Both account fields empty means anonymous access. Web search needs login on the live deployment, while public Wenku downloads and web details/chapters can be read anonymously. Supplying only one account field, a failed login or an invalid token makes authenticated API calls unavailable; the plugin never silently falls back to guest access.

The plugin posts `{app: "n", username, password}` to the configured authentication service's `/api/v1/auth/login` and uses its returned JWT as the API Bearer token. Login is lazy: one submission per export call that needs network data, reused across API requests in that call. Password submissions are never retried. The guest is stateless between calls, so later calls log in afresh rather than keeping an expiring token or browser refresh cookie. Illustration requests receive neither credentials nor the Bearer token.

Only use trusted site and authentication endpoints. Credentials are stored in ordinary instance configuration, **not encrypted by this plugin**; protect administrator access and database backups. The admin form masks password fields visually, which is not a storage security boundary. Real credentials are never supplied as defaults or committed to the repository; login logs omit passwords, tokens and response bodies.

## IDs and content loading

- Web book metadata and content: `web:<providerId>:<novelId>`, e.g. `web:syosetu:n9669bk`.
- Wenku content only: `wenku-volume:<novelId>:<encoded-volumeId>`. Take `novelId` from the series URL and `volumeId` exactly from `volumeJp[].volumeId` in the site's API. Each component is UTF-8 percent encoded. Plain Unicode filenames are also accepted when they contain no reserved colon/percent characters; the returned ID is canonicalized.

For example, this live upload can be addressed as:

```text
wenku-volume:6806f70f2d549f092c9ae862:筺底のエルピス5　－迷い子たちの一歩－.epub
```

`volumeId` is the upstream upload filename, now explicitly used as identity. It is not an ASIN, inferred title, list index or filename hash. Exact filename spelling is preserved: no fuzzy, width or whitespace matching. Reordering uploads does not change an ID; renaming/removing an upload makes the old ID unavailable. Duplicate upload IDs fail closed. The plugin only reads the upload list and translation availability, ignoring bibliographic `volumes`, ASINs, titles and covers. Admin-only Chinese uploads (`volumeZh`) are not exposed.

Wenku declares file-mode content and returns a translated EPUB/TXT through `get-book-file`; it does not return book or series metadata. Web novels declare chapter-mode content: TOC headings without chapter IDs are omitted, and each chapter call resolves its index to the upstream chapter ID. Prose is escaped HTML; original `<图片>URL` markers become stored-image figures (failed image downloads retain the HTTP(S) URL).

Web automatic mode chooses the first complete translation array, falling back to Japanese. A fixed translator with no complete array returns unavailable; empty chapters remain valid. Wenku requires a positive chapter count for a selected translator and does not support Japanese-only downloads. Downloads use `mode=zh&translationsMode=priority` with repeated `translations` parameters. Partial translations remain partial: the upstream exporter retains original EPUB chapters or inserts missing-translation notes in TXT. The plugin never starts translation jobs.

Translation settings are not encoded into content IDs. Changing the translator or upstream file can change the returned bytes without changing the ID; rematerialize cached content when necessary. Search totals are estimates until the final upstream page. GET transport/timeouts and 5xx have at most one retry; 401/403/404/429 are final. Host fetch size/time caps apply to large downloads.

## Upgrade from earlier builds

`catalog`, `access-token` and `volume-files` have been removed. Save the new configuration with username/password if login is needed. `wenku:<seriesId>` metadata lookups and `wenku-asin:...` IDs are no longer supported; reattach Wenku content using the actual upload's `volumeId`. Existing local metadata and cached bodies are not deleted or migrated automatically.

## Verification

```sh
cargo test -p novelia-plugin
./scripts/build-plugins.sh novelia
cargo test -p mandara-plugin --test novelia compiled_guest -- --ignored
# Opt-in live validation (downloads one translated EPUB):
cargo test -p mandara-plugin --test novelia live_guest -- --ignored
```

Mock integration tests exercise the actual Wasm component, login JSON/password whitespace, token use, one login per call, failed-login behavior, illustration isolation, content-only Wenku lookups and file redirects. A captured upload-list fixture tests exact volumeId addressing independently of metadata.

Set `NOVELIA_TEST_USERNAME` and `NOVELIA_TEST_PASSWORD` in the test process environment to also validate the guest's login and authenticated web search. Never persist real credentials in test fixtures or shell history. Live tests are opt-in because network availability, permissions and upstream catalog data can change.

The upstream contract was inspected at [cbfd3f8](https://github.com/auto-novel/auto-novel/tree/cbfd3f8bf754d848c7401812b3922ad3677cc355): [web API](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/web/src/api/WebNovelApi.ts) and [Wenku routes](https://github.com/auto-novel/auto-novel/blob/cbfd3f8bf754d848c7401812b3922ad3677cc355/server/src/main/kotlin/api/RouteWenkuNovel.kt). Login uses the [official authentication API](https://github.com/auto-novel/auth/blob/8a7152d1965fa14921941948b918a7a9e7431cd4/apps/api/internal/service/auth/auth.go). Upload fields were checked against [live Wenku data](https://n.novelia.cc/api/wenku/6806f70f2d549f092c9ae862).
