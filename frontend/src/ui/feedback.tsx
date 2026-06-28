import { AlertTriangle, CircleCheck, Info, LoaderCircle } from "./icons";

type Tone = "error" | "success" | "info";

const toneStyles: Record<Tone, { wrap: string; icon: React.ComponentType<{ className?: string }> }> = {
    error: {
        wrap: "border-clay/30 bg-clay/10 text-clay",
        icon: AlertTriangle,
    },
    success: {
        wrap: "border-success/30 bg-success/10 text-success",
        icon: CircleCheck,
    },
    info: {
        wrap: "border-wisteria/30 bg-wisteria/10 text-wisteria-deep",
        icon: Info,
    },
};

/** An inline feedback banner for errors, successes and notices. Replaces the
 *  former bespoke error/success blocks with one tone-driven component. */
export function Banner({
    tone = "info",
    children,
}: {
    tone?: Tone;
    children: React.ReactNode;
}) {
    const { wrap, icon: Icon } = toneStyles[tone];
    return (
        <div
            role={tone === "error" ? "alert" : "status"}
            className={`flex items-start gap-3 rounded-lg border px-4 py-3 text-sm ${wrap}`}
        >
            <Icon className="mt-0.5 h-4 w-4 shrink-0" />
            <div className="min-w-0 flex-1">{children}</div>
        </div>
    );
}

/** A spinning loader, sized via `className`. */
export function Spinner({ className = "h-5 w-5" }: { className?: string }) {
    return (
        <LoaderCircle
            className={`animate-spin text-ink-faint ${className}`}
            aria-hidden
        />
    );
}

/** A centered loading state with spinner and label, for full-section waits. */
export function Loading({ label = "Loading…" }: { label?: string }) {
    return (
        <div className="flex items-center gap-2 text-sm text-ink-soft" role="status">
            <Spinner className="h-4 w-4" />
            <span>{label}</span>
        </div>
    );
}

/** A neutral shimmer placeholder used while content loads. */
export function Skeleton({ className = "" }: { className?: string }) {
    return (
        <div
            className={`animate-pulse rounded-md bg-line/70 ${className}`}
            aria-hidden
        />
    );
}

/** A centered empty-state with an icon, message and optional action. */
export function EmptyState({
    icon: Icon,
    title,
    description,
    action,
}: {
    icon: React.ComponentType<{ className?: string }>;
    title: string;
    description?: string;
    action?: React.ReactNode;
}) {
    return (
        <div className="flex flex-col items-center gap-2 rounded-2xl border border-dashed border-line bg-surface/50 px-6 py-12 text-center">
            <Icon className="h-8 w-8 text-ink-faint" />
            <p className="text-sm font-medium text-ink">{title}</p>
            {description && <p className="max-w-sm text-sm text-ink-soft">{description}</p>}
            {action && <div className="mt-2">{action}</div>}
        </div>
    );
}
