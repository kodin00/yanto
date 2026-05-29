import { Check, ChevronDown, Loader2, X } from "lucide-react";
import type { ReactNode } from "react";
import { useEffect, useRef, useState } from "react";

type ButtonProps = {
  children: ReactNode;
  onClick?: () => void;
  type?: "button" | "submit";
  variant?: "primary" | "secondary" | "danger" | "ghost";
  disabled?: boolean;
  icon?: ReactNode;
};

export function Button({ children, onClick, type = "button", variant = "primary", disabled, icon }: ButtonProps) {
  return (
    <button className={`button ${variant}`} type={type} onClick={onClick} disabled={disabled}>
      {icon}
      <span>{children}</span>
    </button>
  );
}

export function IconButton({ label, onClick, children, variant = "ghost" }: { label: string; onClick: () => void; children: ReactNode; variant?: ButtonProps["variant"] }) {
  return (
    <button className={`icon-button ${variant}`} type="button" onClick={onClick} aria-label={label} title={label}>
      {children}
    </button>
  );
}

export function TextField({
  label,
  value,
  onChange,
  placeholder,
  type = "text",
  required,
  autoComplete
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: "text" | "password";
  required?: boolean;
  autoComplete?: string;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      <input value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} type={type} required={required} autoComplete={autoComplete} />
    </label>
  );
}

export function TextAreaField({
  label,
  value,
  onChange,
  placeholder
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
}) {
  return (
    <label className="field text-area-field">
      <span>{label}</span>
      <textarea value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} spellCheck={false} />
    </label>
  );
}

export function ToggleField({ label, value, onChange, description }: { label: string; value: boolean; onChange: (value: boolean) => void; description?: string }) {
  return (
    <div className="toggle-field">
      <div>
        <span>{label}</span>
        {description ? <p>{description}</p> : null}
      </div>
      <button type="button" role="switch" aria-checked={value} className={`toggle-switch ${value ? "on" : ""}`} onClick={() => onChange(!value)}>
        <span />
      </button>
    </div>
  );
}

export function CustomSelect<T extends string>({
  label,
  value,
  options,
  onChange
}: {
  label: string;
  value: T;
  options: { label: string; value: T }[];
  onChange: (value: T) => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const close = (event: MouseEvent) => {
      if (!ref.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, []);

  const selected = options.find((option) => option.value === value) ?? options[0];

  return (
    <div className="field custom-select" ref={ref}>
      <span>{label}</span>
      <button type="button" className="select-trigger" onClick={() => setOpen((current) => !current)}>
        <span>{selected.label}</span>
        <ChevronDown size={16} />
      </button>
      {open ? (
        <div className="select-menu">
          {options.map((option) => (
            <button
              type="button"
              key={option.value}
              className={option.value === value ? "selected" : ""}
              onClick={() => {
                onChange(option.value);
                setOpen(false);
              }}
            >
              <span>{option.label}</span>
              {option.value === value ? <Check size={15} /> : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

export function Modal({ title, children, onClose, size = "default", closeOnEscape = true }: { title: string; children: ReactNode; onClose: () => void; size?: "default" | "wide"; closeOnEscape?: boolean }) {
  useEffect(() => {
    if (!closeOnEscape) return;
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", handleEscape);
    return () => window.removeEventListener("keydown", handleEscape);
  }, [closeOnEscape, onClose]);

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) {
        onClose();
      }
    }}>
      <div className={`modal ${size}`} role="dialog" aria-modal="true" aria-label={title} onMouseDown={(event) => event.stopPropagation()}>
        <div className="modal-head">
          <h2>{title}</h2>
          <IconButton label="Close" onClick={onClose}>
            <X size={18} />
          </IconButton>
        </div>
        {children}
      </div>
    </div>
  );
}

export function ConfirmDialog({
  title,
  body,
  confirmLabel,
  danger,
  onConfirm,
  onClose
}: {
  title: string;
  body: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Modal title={title} onClose={onClose}>
      <p className="muted">{body}</p>
      <div className="actions">
        <Button variant="secondary" onClick={onClose}>
          Cancel
        </Button>
        <Button variant={danger ? "danger" : "primary"} onClick={onConfirm}>
          {confirmLabel}
        </Button>
      </div>
    </Modal>
  );
}

export function Toast({ message, kind = "ok", onClose }: { message: string; kind?: "ok" | "error" | "loading"; onClose: () => void }) {
  useEffect(() => {
    if (kind === "loading") return;
    const timer = window.setTimeout(onClose, 4200);
    return () => window.clearTimeout(timer);
  }, [kind, onClose]);

  return (
    <div className={`toast ${kind}`}>
      <span>{message}</span>
      <button type="button" onClick={onClose} aria-label="Dismiss">
        <X size={15} />
      </button>
    </div>
  );
}

export function StatusBadge({ status }: { status: string }) {
  return <span className={`status ${status.toLowerCase()}`}>{status}</span>;
}

export function LogViewer({ logs }: { logs: string }) {
  const ref = useRef<HTMLPreElement | null>(null);

  useEffect(() => {
    const element = ref.current;
    if (element) {
      element.scrollTop = element.scrollHeight;
    }
  }, [logs]);

  return <pre ref={ref} className="log-viewer">{logs || "Waiting for logs..."}</pre>;
}

export function LoadingInline({ label }: { label: string }) {
  return (
    <span className="loading-inline">
      <Loader2 size={15} className="spin" />
      {label}
    </span>
  );
}
