import { createContext, useCallback, useContext, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { LogViewer, Modal, StatusBadge } from "../components/ui";
import type { LogModalState, LogStreamPayload } from "../types";

type LogModalContextValue = {
  openLogModal: (title: string, logs: string) => void;
  openStreamingLog: (title: string, streamPath: string, status?: string) => void;
  closeLogModal: () => void;
};

const LogModalContext = createContext<LogModalContextValue | null>(null);

export function useLogModal() {
  const ctx = useContext(LogModalContext);
  if (!ctx) throw new Error("useLogModal must be inside LogModalProvider");
  return ctx;
}

const MAX_SSE_RETRIES = 3;
const SSE_RETRY_BASE_MS = 1000;

export function LogModalProvider({ children }: { children: ReactNode }) {
  const [logModal, setLogModal] = useState<LogModalState | null>(null);

  const openLogModal = useCallback((title: string, logs: string) => {
    setLogModal({ title, logs });
  }, []);

  const openStreamingLog = useCallback((title: string, streamPath: string, status?: string) => {
    setLogModal({ title, logs: "", streamPath, live: true, status });
  }, []);

  const closeLogModal = useCallback(() => setLogModal(null), []);

  useEffect(() => {
    if (!logModal?.streamPath) return;
    const streamPath = logModal.streamPath;
    let retryCount = 0;
    let source: EventSource | null = null;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;

    function connect() {
      source = new EventSource(streamPath);

      source.onmessage = (event) => {
        retryCount = 0;
        let payload: LogStreamPayload;
        try {
          payload = JSON.parse(event.data) as LogStreamPayload;
        } catch {
          payload = { chunk: event.data };
        }
        setLogModal((current) => {
          if (!current || current.streamPath !== streamPath) return current;
          const nextLogs = payload.logs ?? `${current.logs}${payload.chunk ?? ""}${payload.error ? `\n${payload.error}\n` : ""}`;
          return { ...current, logs: nextLogs, status: payload.status ?? current.status, live: !payload.done };
        });
        if (payload.done) {
          source?.close();
        }
      };

      source.onerror = () => {
        source?.close();
        if (retryCount < MAX_SSE_RETRIES) {
          retryCount++;
          const delay = SSE_RETRY_BASE_MS * Math.pow(2, retryCount - 1);
          retryTimer = setTimeout(connect, delay);
        } else {
          setLogModal((current) => (current?.streamPath === streamPath ? { ...current, live: false } : current));
        }
      };
    }

    connect();

    return () => {
      source?.close();
      if (retryTimer) clearTimeout(retryTimer);
    };
  }, [logModal?.streamPath]);

  return (
    <LogModalContext.Provider value={{ openLogModal, openStreamingLog, closeLogModal }}>
      {children}
      {logModal ? (
        <Modal title={logModal.title} onClose={closeLogModal}>
          {logModal.streamPath ? (
            <div className="log-status-line">
              <StatusBadge status={logModal.live ? "live" : logModal.status ?? "closed"} />
              <span>{logModal.live ? "Streaming logs" : "Log stream closed"}</span>
            </div>
          ) : null}
          <LogViewer logs={logModal.logs} />
        </Modal>
      ) : null}
    </LogModalContext.Provider>
  );
}
