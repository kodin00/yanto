type LogLevel = "info" | "warn" | "error";

const historyLimit = 500;
const history: string[] = [];

function write(level: LogLevel, message: string, meta?: Record<string, unknown>) {
  const payload = {
    level,
    message,
    time: new Date().toISOString(),
    ...meta
  };
  const line = JSON.stringify(payload);
  history.push(line);
  if (history.length > historyLimit) {
    history.splice(0, history.length - historyLimit);
  }
  if (level === "error") {
    console.error(line);
    return;
  }
  if (level === "warn") {
    console.warn(line);
    return;
  }
  console.log(line);
}

export const logger = {
  info: (message: string, meta?: Record<string, unknown>) => write("info", message, meta),
  warn: (message: string, meta?: Record<string, unknown>) => write("warn", message, meta),
  error: (message: string, meta?: Record<string, unknown>) => write("error", message, meta),
  history: () => history.join("\n")
};
