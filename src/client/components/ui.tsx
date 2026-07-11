import { Check, ChevronDown, Loader2, X } from "lucide-react";
import type { ReactNode } from "react";
import { useEffect, useId, useRef, useState } from "react";

type ButtonProps = {
  children: ReactNode;
  onClick?: () => void;
  type?: "button" | "submit";
  variant?: "primary" | "secondary" | "danger" | "ghost";
  disabled?: boolean;
  icon?: ReactNode;
  loading?: boolean;
};

export function Button({ children, onClick, type = "button", variant = "primary", disabled, icon, loading }: ButtonProps) {
  return (
    <button className={`button ${variant}`} type={type} onClick={onClick} disabled={disabled || loading}>
      {loading ? <Loader2 size={15} className="spin" /> : icon}
      <span>{children}</span>
    </button>
  );
}

export function IconButton({ label, onClick, children, variant = "ghost", disabled }: { label: string; onClick: () => void; children: ReactNode; variant?: ButtonProps["variant"]; disabled?: boolean }) {
  return (
    <button className={`icon-button ${variant}`} type="button" onClick={onClick} aria-label={label} title={label} disabled={disabled}>
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
  autoComplete,
  disabled
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: "text" | "password";
  required?: boolean;
  autoComplete?: string;
  disabled?: boolean;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      <input value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} type={type} required={required} autoComplete={autoComplete} disabled={disabled} />
    </label>
  );
}

export function TextAreaField({
  label,
  value,
  onChange,
  placeholder,
  compact = false,
  autoGrow = false,
  disabled = false
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  compact?: boolean;
  autoGrow?: boolean;
  disabled?: boolean;
}) {
  const ref = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    if (!autoGrow || !ref.current) return;
    ref.current.style.height = "auto";
    ref.current.style.height = `${ref.current.scrollHeight}px`;
  }, [autoGrow, value]);

  return (
    <label className={`field text-area-field ${compact ? "compact" : ""} ${autoGrow ? "auto-grow" : ""}`}>
      <span>{label}</span>
      <textarea ref={ref} value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} spellCheck={false} disabled={disabled} rows={compact ? 1 : undefined} />
    </label>
  );
}

export function ToggleField({ label, value, onChange, description, disabled }: { label: string; value: boolean; onChange: (value: boolean) => void; description?: string; disabled?: boolean }) {
  return (
    <div className="toggle-field">
      <div>
        <span>{label}</span>
        {description ? <p>{description}</p> : null}
      </div>
      <button type="button" role="switch" aria-checked={value} className={`toggle-switch ${value ? "on" : ""}`} onClick={() => onChange(!value)} disabled={disabled}>
        <span />
      </button>
    </div>
  );
}

export function CustomSelect<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  placeholder = "No options available"
}: {
  label: string;
  value: T;
  options: { label: string; value: T }[];
  onChange: (value: T) => void;
  disabled?: boolean;
  placeholder?: string;
}) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const ref = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const menuId = useId();
  const labelId = useId();

  useEffect(() => {
    const close = (event: MouseEvent) => {
      if (!ref.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, []);

  const selectedIndex = options.findIndex((option) => option.value === value);
  const selected = options[selectedIndex] ?? options[0];
  const unavailable = disabled || options.length === 0;

  useEffect(() => {
    if (unavailable) setOpen(false);
  }, [unavailable]);

  useEffect(() => {
    if (!open) return;
    const nextIndex = selectedIndex >= 0 ? selectedIndex : 0;
    setActiveIndex(nextIndex);
    const frame = window.requestAnimationFrame(() => optionRefs.current[nextIndex]?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [open, selectedIndex]);

  const focusOption = (index: number) => {
    if (!options.length) return;
    const wrapped = (index + options.length) % options.length;
    setActiveIndex(wrapped);
    optionRefs.current[wrapped]?.focus();
  };

  const closeAndFocus = () => {
    setOpen(false);
    window.requestAnimationFrame(() => triggerRef.current?.focus());
  };

  return (
    <div className="field custom-select" ref={ref}>
      <span id={labelId}>{label}</span>
      <button
        ref={triggerRef}
        type="button"
        className="select-trigger"
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === "Escape") closeAndFocus();
          if ((event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Enter" || event.key === " ") && !open) {
            event.preventDefault();
            setOpen(true);
          }
        }}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={menuId}
        aria-labelledby={labelId}
        disabled={unavailable}
      >
        <span>{selected?.label ?? placeholder}</span>
        <ChevronDown className="select-chevron" size={16} />
      </button>
      {open ? (
        <div
          className="select-menu"
          id={menuId}
          role="listbox"
          aria-labelledby={labelId}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") { event.preventDefault(); focusOption(activeIndex + 1); }
            if (event.key === "ArrowUp") { event.preventDefault(); focusOption(activeIndex - 1); }
            if (event.key === "Home") { event.preventDefault(); focusOption(0); }
            if (event.key === "End") { event.preventDefault(); focusOption(options.length - 1); }
            if (event.key === "Escape") { event.preventDefault(); closeAndFocus(); }
            if (event.key === "Tab") setOpen(false);
          }}
        >
          {options.map((option, index) => (
            <button
              ref={(element) => { optionRefs.current[index] = element; }}
              type="button"
              key={option.value}
              className={option.value === value ? "selected" : ""}
              role="option"
              aria-selected={option.value === value}
              tabIndex={index === activeIndex ? 0 : -1}
              onClick={() => {
                onChange(option.value);
                closeAndFocus();
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

export function StatusBadge({ status, label, title }: { status: string; label?: string; title?: string }) {
  const normalized = status.toLowerCase().replace(/[^a-z0-9-]+/g, "-");
  const displayLabel = label ?? status.replace(/[-_]+/g, " ");
  return <span className={`status ${normalized}`} title={title}>{displayLabel}</span>;
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
