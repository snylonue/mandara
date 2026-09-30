import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api";
import { useAuth } from "../auth";
import { LogoMark } from "../components/icons";
import type { AuthResp } from "../types";

function AuthForm({ mode }: { mode: "login" | "register" }) {
  const { login } = useAuth();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const resp = await api<AuthResp>(`/auth/${mode}`, {
        method: "POST",
        body: JSON.stringify({ username, password }),
      });
      login(resp.token, resp.user);
      navigate("/", { replace: true });
    } catch (err) {
      setError(err instanceof Error ? err.message : t("library.requestFailed"));
    } finally {
      setBusy(false);
    }
  }

  const isLogin = mode === "login";

  return (
    <div className="auth-wrap">
      <form className="card auth-card" onSubmit={submit}>
        <div className="auth-brand">
          <LogoMark size={30} className="brand-mark" />
        </div>
        <h1>{isLogin ? t("auth.loginTitle") : t("auth.registerTitle")}</h1>
        <label>
          {t("auth.username")}
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="username"
            required
            minLength={3}
          />
        </label>
        <label>
          {t("auth.password")}
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete={isLogin ? "current-password" : "new-password"}
            required
            minLength={8}
          />
        </label>
        {error && <div className="error">{error}</div>}
        <button className="btn primary" disabled={busy}>
          {isLogin ? t("auth.submitLogin") : t("auth.submitRegister")}
        </button>
        <p className="hint">
          {isLogin ? (
            <>
              {t("auth.noAccount")} <Link to="/register">{t("auth.toRegister")}</Link>
            </>
          ) : (
            <>
              {t("auth.haveAccount")} <Link to="/login">{t("auth.toLogin")}</Link>
            </>
          )}
        </p>
      </form>
      <p className="hint center">{t("auth.tagline")}</p>
    </div>
  );
}

export function LoginPage() {
  return <AuthForm mode="login" />;
}

export function RegisterPage() {
  return <AuthForm mode="register" />;
}