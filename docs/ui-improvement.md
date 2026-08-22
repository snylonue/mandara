# Frontend UI Improvement — Design & Plan

Status: **proposal**.

Motivation: the current frontend works but is visually under-polished
(reviewed against `data/screenshot/*.png`). This document records the
improvement plan and the decisions taken. It is frontend-first and, except
for item 9 (cover storage), touches no API or backend contract. i18n stays
zh-CN only; all new user-facing strings go into
`frontend/src/i18n/locales/zh-CN.json`.

## 1. Problem summary

| Screenshot | Issue |
|---|---|
| `image4.png` | Upload dialog and "source browser" are **inline divs**, not modals — no backdrop, no layering, they overlap each other and the layout. |
| `image3.png` | Book cards are text-only gray boxes (no cover, no visual hierarchy); repetitive text tags (`plugin · plugin · public`) wrap awkwardly; large empty area. |
| `image1.png` / `image2.png` | Reader has no settings (font size / line height / theme / width); the TOC is a heavy inline box with a scrollbar and no current-chapter highlight; prev/next appear twice (top and bottom). |

Bug: `styles.css` references `--panel` and `--card-bg` in `:root`, but they
are **not defined**. `.toc-panel` and `.epub-content pre` get a transparent
background; `.config-form` falls back to `#fafafa` (a white block on the
dark theme).

## 2. Foundation (P0)

1. **Design tokens.** Finish `:root` with the missing `--panel` /
   `--card-bg`, plus a semantic palette (`--surface`, `--surface-raised`,
   `--text-strong`, `--text-muted`, `--border-strong`, `--accent-hover`), a
   spacing scale (4/8/12/16/24), and a type scale. Remove every light-color
   fallback like `var(--x, #fafafa)`.
2. **Buttons.** One component with `primary / default / ghost / danger`
   variants and `sm / md` sizes; add `:focus-visible` rings and
   hover/active transitions.
3. **Reader nav.** Keep one prev/next control set (bottom, large) and move
   "目录" + "返回" to the top; drop the duplicated top controls.

## 3. Modal system (P1)

- Add a shared `Modal` component (backdrop, centered, scroll lock, ESC and
  backdrop-click to close, focus trap). The upload dialog and source browser
  are the first consumers.
- Opening one closes the other, so only a single modal is ever visible.
- Restructure the upload form into clear sections (metadata mode / file
  info) with `grid` spacing instead of one packed row.

## 4. Reader experience (P2)

- **Settings bar:** font size −/+, line height, theme (dark / sepia / light),
  font family (serif / sans), and reading column width. Persist to
  `localStorage`; no backend involvement.
- **Body typography:** `.epub-content` → `max-width: 42em`, `line-height: 2.0`,
  `p`/`div` spacing `0.8em`; do not force `h1 { text-align: center }`.
- **TOC:** replace the `toc-panel` box with a left slide-in drawer — current
  chapter highlighted and auto-scrolled into view.
- **Top bar:** fixed "book title · chapter title · percent"; bottom large
  prev/next buttons preview the next chapter title.

## 5. Library & covers (P3)

- **Cards:** 2:3 cover on top, title (ellipsis), author · chapter count, and
  small icon badges for "公开 / 隐藏 / plugin" instead of the repeated text
  tags. Skeleton loaders and a friendly empty state.
- **Item 9 — store covers (decided: backend storage).** Persist the cover
  image rather than generating a placeholder:
  - `books` gains `cover BLOB` (nullable) and `cover_mime TEXT` (migration
    `0005`). `cover_url` stays as-is for remote/plugin-provided covers.
  - The epub parser already identifies cover bytes; wire them into
    `ParsedBook` (add `cover: Option<Vec<u8>>` + mime) and store during
    upload. `txt` (no cover) keeps `NULL`.
  - New endpoint `GET /api/books/{id}/cover` serves the bytes with the stored
    `Content-Type`; `BookCard` and the detail page render it directly.
  - Update `docs/api/openapi.yaml` accordingly; regenerate frontend types
    (`npm run api-types`); `src/types.ts` re-exports.

Rationale: covers come from the source file, are small, are single-use (one
per metadata entry), and storing them in the row keeps each book
self-contained and lands a large visual gain with a small surface change.

## 6. Global polish (P4)

- Toast notifications (success / error / warning) replace scattered inline
  `.error` boxes; inline `.error` stays for form validation only.
- Nav bar: SVG logo (drop the emoji), nav items (书架 / 插件管理), and a
  user dropdown.
- Inline SVG icon set replaces `←/→/＋/📍` characters and emojis.
- `data-theme` theme switch built on the P0 tokens (default dark).

## 7. Suggested commits

Frontend-only except the cover item. Per the repo convention, one feature =
one commit, and `cargo fmt` lands only in its own `chore:` commit.

1. `style(web): design tokens + fix undefined --panel/--card-bg + button/focus variants`
2. `feat(web): reusable modal, modalize upload + source browser`
3. `feat(web): reader settings (font/theme/width) + TOC drawer + body typography`
4. `feat(web): library cards with covers, icon badges, skeletons, empty state`
5. `feat(api/cover): store epub cover bytes + GET /api/books/{id}/cover` (migration + openapi + types + frontend binding)
6. `feat(web): toasts, nav branding/icons, theme switcher`
