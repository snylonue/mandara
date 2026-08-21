import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";
import type { AuthResp } from "../types";

function AuthForm({
  mode,
}: {
  mode: "login" | "register";
}) {
  const { health, login } = useAuth();
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
      setError(err instanceof Error ? err.message : "请求失败");
    } finally {
      setBusy(false);
    }
  }

  if (health && !health.auth_enabled) {
    return (
      <div className="card">
        <p>服务器以本地模式运行（未启用登录）。</p>
        <Link to="/">返回书架</Link>
      </div>
    );
  }

  return (
    <div className="auth-wrap">
      <form className="card auth-card" onSubmit={submit}>
        <h1>{mode === "login" ? "登录" : "注册"}</h1>
        <label>
          用户名
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="username"
            required
            minLength={3}
          />
        </label>
        <label>
          密码
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete={mode === "login" ? "current-password" : "new-password"}
            required
            minLength={8}
          />
        </label>
        {error && <div className="error">{error}</div>}
        <button className="primary" disabled={busy}>
          {mode === "login" ? "登录" : "注册"}
        </button>
        <p className="hint">
          {mode === "login" ? (
            <>
              还没有账号？<Link to="/register">去注册</Link>
            </>
          ) : (
            <>
              已有账号？<Link to="/login">去登录</Link>
            </>
          )}
        </p>
      </form>
      <p className="hint center">支持 epub / txt 格式的轻小说阅读站</p>
    </div>
  );
}

export function LoginPage() {
  const { health } = useAuth();
  if (health?.auth_enabled === false) return <AuthForm mode="login" />;
  return <AuthForm mode="login" />;
}

export function RegisterPage() {
  const { health } = useAuth();
  if (health?.auth_enabled === false) return <AuthForm mode="register" />;
  return <AuthForm mode="register" />;
}