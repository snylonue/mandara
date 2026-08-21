// API types (mirror of the rust models).

export interface Health {
  status: string;
  version: string;
  auth_enabled: boolean;
  allow_register: boolean;
}

export type Role = "admin" | "user";
export interface User {
  id: string;
  username: string;
  role: Role;
  created_at: string;
}

export interface AuthResp {
  token: string;
  user: User;
}

export type Visibility = "private" | "public";

export interface BookMeta {
  id: string;
  source: string;
  external_id: string;
  title: string;
  authors: string[];
  description: string | null;
  cover_url: string | null;
  visibility: Visibility;
  owner_id: string | null;
  chapter_count: number;
  created_at: string;
}

export interface ChapterMeta {
  idx: number;
  title: string;
}

export interface BookDetail {
  book: BookMeta;
  chapters: ChapterMeta[];
}

export interface Chapter {
  idx: number;
  title: string;
  content: string;
}

export interface Position {
  chapter_idx: number;
  offset: number;
  fraction: number;
}

export interface ReadingSession {
  id: string;
  user_id: string;
  book_id: string;
  label: string;
  position: Position;
  updated_at: string;
}

export interface SessionsResponse {
  book_id: string;
  book_title: string;
  sessions: ReadingSession[];
}

export interface ShareInfo {
  token: string;
  url: string;
  kind: "book" | "session";
  mode: "read" | "progress";
  book_id: string;
  session_id: string | null;
  expires_at: string | null;
  created_at: string;
}

export interface ShareBookView {
  id: string;
  title: string;
  authors: string[];
  description: string | null;
  chapter_count: number;
}

export interface ShareSessionView {
  id: string;
  label: string;
  owner_username: string;
  position: Position;
  percent: number;
  updated_at: string;
}

export interface ShareView {
  kind: "book" | "session";
  mode: "read" | "progress";
  book: ShareBookView;
  session: ShareSessionView | null;
  created_at: string;
  expires_at: string | null;
}

export interface ShareBookResponse {
  book: ShareBookView;
  chapters: ChapterMeta[];
}