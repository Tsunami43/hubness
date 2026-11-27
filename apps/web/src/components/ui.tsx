import { useEffect, useRef, type ButtonHTMLAttributes, type ReactNode } from 'react';
import { Link, useLocation } from 'react-router';
import { Check, ChevronLeft, Loader2, PanelRight, X } from 'lucide-react';
import type { SessionStatus } from '@hubness/shared';
import { statusLabel } from '../lib/format';
import { useNav } from './Layout';
import { t } from '../i18n';
import { PlacesStrip } from './PlacesStrip';
import { usePlaces } from '../lib/places';

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger';

const VARIANTS: Record<Variant, string> = {
  primary: 'btn-accent',
  secondary: 'border border-line hover:border-muted hover:bg-sunken',
  ghost: 'border border-transparent text-muted hover:text-fg hover:bg-sunken',
  danger: 'border border-danger/50 text-danger hover:bg-danger/10',
};

export function Button({
  variant = 'secondary',
  size = 'md',
  loading,
  className = '',
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: 'sm' | 'md'; loading?: boolean }) {
  // touch screens: finger-sized heights (the desktop keeps the dense TUI sizes)
  const sz = size === 'sm' ? 'h-7 px-2 text-[13px] gap-1.5 pointer-coarse:h-10 pointer-coarse:px-3' : 'h-9 px-3 gap-2 pointer-coarse:h-11';
  return (
    <button
      {...rest}
      disabled={rest.disabled || loading}
      className={`ui-btn inline-flex items-center justify-center whitespace-nowrap transition disabled:opacity-50 ${sz} ${VARIANTS[variant]} ${className}`}
    >
      {loading ? <Loader2 className="size-4 animate-spin" /> : null}
      {children}
    </button>
  );
}

export function IconButton({
  label,
  children,
  className = '',
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      {...rest}
      aria-label={label}
      title={label}
      className={`ui-icon tap inline-flex size-9 shrink-0 items-center justify-center text-muted hover:bg-sunken hover:text-fg disabled:opacity-40 ${className}`}
    >
      {children}
    </button>
  );
}

/** Top bar of a screen (TUI): back, title and subtitle inline, actions on the right. */
export function TopBar({
  title,
  subtitle,
  back,
  actions,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  back?: string;
  actions?: ReactNode;
}) {
  const { toggleHistory, historyOpen } = useNav();
  // on desktop the open place is already its tab in the strip: no second title (nor subtitle)
  const places = usePlaces();
  const { pathname } = useLocation();
  const inPlace = places.some((p) => pathname === p.path || pathname.startsWith(`${p.path}/`));
  return (
    <header className="sticky top-0 z-20 flex h-11 shrink-0 items-center gap-2 border-b border-line bg-surface px-2 pointer-coarse:h-[52px] pointer-coarse:gap-1 pointer-coarse:px-1">
      {back ? (
        <Link to={back} aria-label={t('common.back')} className="tap inline-flex size-8 shrink-0 items-center justify-center text-muted hover:bg-sunken hover:text-fg">
          <ChevronLeft className="size-4 pointer-coarse:size-5" />
        </Link>
      ) : (
        <span className="w-1" />
      )}
      <div className={`flex min-w-0 flex-1 items-baseline gap-2 lg:max-w-[40%] lg:flex-initial ${inPlace ? 'lg:hidden' : ''}`}>
        <div className="min-w-0 shrink truncate font-semibold">{title}</div>
        {subtitle ? <div className="hidden min-w-0 shrink truncate text-xs text-muted sm:block">{subtitle}</div> : null}
      </div>
      {/* open projects / groups / chats, like browser tabs (desktop) */}
      <PlacesStrip />
      <span className="hidden flex-1 lg:block" />
      <div className="flex shrink-0 items-center gap-1 pointer-coarse:gap-0">
        {actions}
        <HistoryButton open={historyOpen} onClick={toggleHistory} />
      </div>
    </header>
  );
}

/** Header inside a panel (session in a project tab): back, title, subtitle, actions. */
export function PanelHeader({ title, subtitle, onBack, actions }: { title: ReactNode; subtitle?: ReactNode; onBack?: () => void; actions?: ReactNode }) {
  return (
    <div className="flex h-9 shrink-0 items-center gap-1.5 border-b border-line bg-surface pr-1 pl-1 pointer-coarse:h-[52px] pointer-coarse:gap-1 pointer-coarse:px-1">
      {onBack ? (
        <button type="button" onClick={onBack} aria-label={t('common.back')} className="tap inline-flex size-7 shrink-0 items-center justify-center text-muted hover:bg-sunken hover:text-fg">
          <ChevronLeft className="size-4 pointer-coarse:size-5" />
        </button>
      ) : null}
      <div className="flex min-w-0 flex-1 items-baseline gap-2 pl-1">
        <span className="min-w-0 shrink truncate text-[13px] font-semibold pointer-coarse:text-[15px]">{title}</span>
        {subtitle ? <span className="min-w-0 shrink truncate text-xs text-muted">{subtitle}</span> : null}
      </div>
      {actions}
    </div>
  );
}

/** Button for the right panel with all sessions (top right corner). */
export function HistoryButton({ open, onClick, className = '' }: { open: boolean; onClick: () => void; className?: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={t('history.title')}
      aria-pressed={open}
      title={t('history.title')}
      className={`tap inline-flex size-9 shrink-0 items-center justify-center hover:bg-sunken ${open ? 'text-accent' : 'text-muted hover:text-fg'} ${className}`}
    >
      <PanelRight className="size-4" />
    </button>
  );
}

/** Modal: bottom sheet on phone, centered on desktop. */
export function Sheet({
  open,
  onClose,
  title,
  children,
  footer,
  wide,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  /** Wider on desktop (terminal). */
  wide?: boolean;
}) {
  const downOnBackdrop = useRef(false);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    // phone: a full-screen page (no half-screen popups); desktop: a centered dialog
    <div
      className="inset-app z-50 flex justify-center bg-black/40 sm:items-center sm:p-6"
      // a click on the backdrop closes — not a drag that began inside (selecting text and letting go outside)
      onMouseDown={(e) => (downOnBackdrop.current = e.target === e.currentTarget)}
      onClick={(e) => {
        if (e.target === e.currentTarget && downOnBackdrop.current) onClose();
        downOnBackdrop.current = false;
      }}
    >
      <div
        role="dialog"
        aria-label={title}
        className={`ui-sheet pt-safe flex h-app w-full motion-safe:animate-slide-in-up flex-col bg-surface sm:motion-safe:animate-none sm:h-auto sm:max-h-[90dvh] sm:border sm:border-line sm:pt-0 sm:shadow-2xl sm:shadow-black/50 ${wide ? 'sm:max-w-3xl' : 'sm:max-w-lg'}`}
      >
        <div className="flex shrink-0 items-center gap-2 border-b border-line py-1.5 pr-1.5 pl-4">
          <h2 className="flex-1 font-semibold text-accent">{title}</h2>
          <span className="hidden text-xs text-muted sm:inline">esc</span>
          <IconButton label={t('common.close')} onClick={onClose} className="size-8">
            <X className="size-4" />
          </IconButton>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:flex-initial">{children}</div>
        {footer ? <div className="flex shrink-0 items-center gap-2 border-t border-line px-4 pt-3 pb-[max(0.75rem,var(--safe-b))]">{footer}</div> : null}
      </div>
    </div>
  );
}

const DOT: Record<SessionStatus, string> = {
  idle: 'bg-muted/40',
  running: 'bg-ok animate-pulse',
  waiting: 'bg-warn',
  error: 'bg-danger',
};

export function StatusDot({ status }: { status: SessionStatus }) {
  return <span title={statusLabel(status)} className={`inline-block size-2 shrink-0 rounded-full ${DOT[status]}`} />;
}

export function StatusBadge({ status }: { status: SessionStatus }) {
  if (status === 'idle') return null;
  const color = status === 'running' ? 'text-ok' : status === 'waiting' ? 'text-warn' : 'text-danger';
  return (
    <span className={`inline-flex items-center gap-1.5 text-xs font-medium ${color}`}>
      <StatusDot status={status} />
      {statusLabel(status)}
    </span>
  );
}

export function Empty({ icon, title, hint, action }: { icon?: ReactNode; title: string; hint?: string; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center px-6 py-16 text-center">
      {icon ? <div className="mb-3 text-muted">{icon}</div> : null}
      <div className="text-fg">{title}</div>
      {hint ? <div className="mt-1 max-w-sm text-sm text-muted">{hint}</div> : null}
      {action ? <div className="mt-4">{action}</div> : null}
    </div>
  );
}

export function Spinner({ className = '' }: { className?: string }) {
  return <Loader2 className={`size-5 animate-spin text-muted ${className}`} />;
}

/** Placeholder while a lazily loaded screen arrives. */
export function PageSpinner() {
  return (
    <div className="flex flex-1 justify-center py-10">
      <Spinner />
    </div>
  );
}

export function ErrorLine({ error }: { error: string | null | undefined }) {
  if (!error) return null;
  return <div className="border-l-2 border-danger bg-danger/5 px-3 py-1.5 text-sm text-danger">{error}</div>;
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block">
      <div className="mb-1 text-xs text-muted">{label}</div>
      {children}
      {hint ? <div className="mt-1 text-xs text-muted">{hint}</div> : null}
    </label>
  );
}

export const inputCls =
  'ui-input w-full border border-line bg-bg px-2.5 py-1.5 text-[14px] outline-none focus:border-accent placeholder:text-muted/70';

/** Themed checkbox (the native one ignores the theme): a real input, drawn by us. */
export function Checkbox({ checked, onChange, children, disabled }: { checked: boolean; onChange: (v: boolean) => void; children?: ReactNode; disabled?: boolean }) {
  return (
    <label className={`inline-flex items-start gap-2 ${disabled ? 'opacity-50' : 'cursor-pointer'}`}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} className="peer sr-only" />
      <span
        aria-hidden
        className={`mt-0.5 inline-flex size-4 shrink-0 items-center justify-center border peer-focus-visible:outline peer-focus-visible:outline-1 peer-focus-visible:outline-accent ${checked ? 'border-accent bg-accent text-accent-fg' : 'border-line bg-bg'}`}
      >
        {checked ? <Check className="size-3" strokeWidth={3} /> : null}
      </span>
      {children ? <span className="min-w-0">{children}</span> : null}
    </label>
  );
}

/** On/off toggle with a label and an optional hint. */
export function Switch({ checked, onChange, disabled, label, hint }: { checked: boolean; onChange: (v: boolean) => void; disabled?: boolean; label: string; hint?: string }) {
  return (
    <label className={`flex items-start gap-3 ${disabled ? 'opacity-60' : 'cursor-pointer'}`}>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => onChange(!checked)}
        className={`mt-0.5 flex h-5 w-9 shrink-0 items-center rounded-full border transition ${checked ? 'border-accent bg-accent' : 'border-line bg-sunken'}`}
      >
        <span className={`size-3.5 rounded-full bg-surface shadow transition ${checked ? 'translate-x-4.5' : 'translate-x-0.5'}`} />
      </button>
      <span className="min-w-0">
        <span className="block text-sm font-medium">{label}</span>
        {hint ? <span className="block text-xs text-muted">{hint}</span> : null}
      </span>
    </label>
  );
}
