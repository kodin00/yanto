import { createContext, useCallback, useContext, useState } from "react";
import type { ReactNode } from "react";
import { ConfirmDialog } from "../components/ui";
import { useToast } from "./ToastContext";
import type { ConfirmState } from "../types";

type ConfirmContextValue = {
  showConfirm: (state: ConfirmState) => void;
};

const ConfirmContext = createContext<ConfirmContextValue | null>(null);

export function useConfirm() {
  const ctx = useContext(ConfirmContext);
  if (!ctx) throw new Error("useConfirm must be inside ConfirmProvider");
  return ctx;
}

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);
  const { showToast } = useToast();

  const showConfirm = useCallback((state: ConfirmState) => {
    setConfirm(state);
  }, []);

  return (
    <ConfirmContext.Provider value={{ showConfirm }}>
      {children}
      {confirm ? (
        <ConfirmDialog
          title={confirm.title}
          body={confirm.body}
          confirmLabel={confirm.label}
          danger={confirm.danger}
          onClose={() => setConfirm(null)}
          onConfirm={() => {
            const action = confirm.action;
            const loadingMessage = confirm.loadingMessage ?? `${confirm.label} in progress...`;
            const successMessage = confirm.successMessage ?? "Action completed.";
            setConfirm(null);
            showToast(loadingMessage, "loading");
            action()
              .then(() => showToast(successMessage))
              .catch((error) => showToast(error instanceof Error ? error.message : "Action failed.", "error"));
          }}
        />
      ) : null}
    </ConfirmContext.Provider>
  );
}
