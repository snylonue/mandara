// API types — derived from the API contract in
// `../docs/api/openapi.yaml` via openapi-typescript (see `npm run api-types`
// which regenerates `./schema.d.ts`).
//
// Do not hand-mirror backend types here; only re-export convenient aliases
// so pages can use short names.

import type { components } from "./api/schema";

type Schemas = components["schemas"];

export type Health = Schemas["Health"];
export type Role = Schemas["Role"];
export type User = Schemas["User"];
export type AuthResp = Schemas["AuthResponse"];
export type Visibility = Schemas["Visibility"];
export type BookMeta = Schemas["BookMeta"];
export type FileMeta = Schemas["FileMeta"];
export type BookDetail = Schemas["BookDetail"];
export type BookListEntry = Schemas["BookListEntry"];
export type FileDetail = Schemas["FileDetail"];
export type ChapterMeta = Schemas["ChapterMeta"];
export type Chapter = Schemas["Chapter"];
export type Position = Schemas["Position"];
export type ReadingSession = Schemas["ReadingSession"];
export type SessionsResponse = Schemas["SessionsResponse"];
export type ShareKind = Schemas["ShareKind"];
export type ShareInfo = Schemas["ShareResponse"];
export type ShareBookView = Schemas["ShareBookView"];
export type ShareFileView = Schemas["ShareFileView"];
export type ShareSessionView = Schemas["ShareSessionView"];
export type ShareView = Schemas["ShareView"];
export type ShareBookResponse = Schemas["ShareBookResponse"];