import { createContext, useCallback, useContext, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { api } from "../lib/api";
import { useToast } from "./ToastContext";

type AuthContextValue = {
  user: string | null;
  loading: boolean;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
};

const AuthContext = createContext<AuthContextValue | null>(null);

export function useAuth() {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be inside AuthProvider");
  return ctx;
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const { showToast, clearToast } = useToast();

  useEffect(() => {
    api
      .me()
      .then((result) => setUser(result.username))
      .catch(() => setUser(null))
      .finally(() => setLoading(false));
  }, []);

  const login = useCallback(async (username: string, password: string) => {
    showToast("Signing in...", "loading");
    try {
      const result = await api.login(username, password);
      setUser(result.username);
      showToast("Signed in.");
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to sign in.", "error");
      throw error;
    }
  }, [showToast]);

  const logout = useCallback(async () => {
    showToast("Signing out...", "loading");
    try {
      await api.logout();
      setUser(null);
      clearToast();
    } catch (error) {
      showToast(error instanceof Error ? error.message : "Unable to sign out.", "error");
    }
  }, [showToast, clearToast]);

  return (
    <AuthContext.Provider value={{ user, loading, login, logout }}>
      {children}
    </AuthContext.Provider>
  );
}
