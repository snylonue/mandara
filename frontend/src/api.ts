// Typed API client for the bookshelf backend.

const TOKEN_KEY = "bookshelf_token";

export const getToken = (): string | null => localStorage.getItem(TOKEN_KEY);
export const setToken = (t: string | null) => {
  if (t) localStorage.setItem(TOKEN_KEY, t);
  else localStorage.removeItem(TOKEN_KEY);
};

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    /** Raw error body (e.g. `{error, files}` from a 409). */
    public details?: unknown,
  ) {
    super(message);
  }
}

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers: Record<string, string> = {};
  if (!(init.body instanceof FormData)) headers["Content-Type"] = "application/json";
  const token = getToken();
  if (token) headers["Authorization"] = `Bearer ${token}`;
  // Same-origin requests (dev proxy / served dist) automatically carry
  // the session cookie; without it the server falls back to the
  // Authorization header. `credentials: "include"` makes the cookie
  // explicit for the XHR/fetch path.
  const res = await fetch(`/api${path}`, { ...init, headers, credentials: "include" });
  if (!res.ok) {
    let msg = res.statusText;
    let details: unknown;
    try {
      const j = await res.json();
      if (j?.error) msg = j.error;
      details = j;
    } catch {
      /* non-json body */
    }
    if (res.status === 401 && token && !location.pathname.startsWith("/share/")) {
      setToken(null);
      if (location.pathname !== "/login") location.href = "/login";
    }
    throw new ApiError(res.status, msg, details);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

/// Download a binary endpoint (e.g. the retained original at
/// `/files/{id}/download`) as a file, carrying the auth token.
export async function downloadBinary(path: string, filename: string): Promise<void> {
  const headers: Record<string, string> = {};
  const token = getToken();
  if (token) headers["Authorization"] = `Bearer ${token}`;
  const res = await fetch(`/api${path}`, { headers, credentials: "include" });
  if (!res.ok) throw new Error(res.statusText);
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}