import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { api, setToken } from "./api";
import type { Health, User } from "./types";

interface AuthState {
  health: Health | null;
  user: User | null;
  login: (token: string, user: User) => void;
  logout: () => void;
  refresh: () => Promise<void>;
}

const AuthCtx = createContext<AuthState>({
  health: null,
  user: null,
  login: () => {},
  logout: () => {},
  refresh: async () => {},
});

export function AuthProvider({ children }: { children: ReactNode }) {
  const [health, setHealth] = useState<Health | null>(null);
  const [user, setUser] = useState<User | null>(null);

  const refresh = useCallback(async () => {
    const h = await api<Health>("/health").catch(() => null);
    setHealth(h);
    if (!h) return;
    // Cookie or Bearer token: after a page refresh the localStorage
    // token is still there, but the session cookie alone is enough —
    // `/auth/me` accepts either.
    const u = await api<User>("/auth/me").catch(() => null);
    setUser(u);
    if (!u) setToken(null);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const login = useCallback((token: string, u: User) => {
    setToken(token);
    setUser(u);
  }, []);

  const logout = useCallback(() => {
    setToken(null);
    setUser(null);
    // Clear the server-side session cookie (best-effort).
    void api("/auth/logout", { method: "POST" }).catch(() => undefined);
  }, []);

  return (
    <AuthCtx.Provider value={{ health, user, login, logout, refresh }}>
      {children}
    </AuthCtx.Provider>
  );
}

export function useAuth(): AuthState {
  return useContext(AuthCtx);
}