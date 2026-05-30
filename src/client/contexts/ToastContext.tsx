import { createContext, useCallback, useContext, useState } from "react";
import type { ReactNode } from "react";
import { Toast } from "../components/ui";
import type { ToastState } from "../types";

type ToastContextValue = {
  showToast: (message: string, kind?: "ok" | "error" | "loading") => void;
  clearToast: () => void;
};

const ToastContext = createContext<ToastContextValue | null>(null);

export function useToast() {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be inside ToastProvider");
  return ctx;
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toast, setToast] = useState<ToastState>(null);

  const showToast = useCallback((message: string, kind?: "ok" | "error" | "loading") => {
    setToast({ message, kind: kind ?? "ok" });
  }, []);

  const clearToast = useCallback(() => setToast(null), []);

  return (
    <ToastContext.Provider value={{ showToast, clearToast }}>
      {children}
      {toast ? <Toast {...toast} onClose={clearToast} /> : null}
    </ToastContext.Provider>
  );
}
