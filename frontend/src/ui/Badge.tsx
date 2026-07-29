export type BadgeTone = "neutral" | "cream" | "wisteria";

// Keyed by the tone union, and built once at module load rather than per render.
const TONE_CLASS: Record<BadgeTone, string> = {
    neutral: "bg-canvas text-ink-soft ring-1 ring-line",
    cream: "bg-cream/60 text-ink ring-1 ring-cream",
    wisteria: "bg-wisteria/15 text-wisteria-deep ring-1 ring-wisteria/30",
};

/** A small status pill, e.g. a version marker or role indicator. */
export function Badge({
    tone = "neutral",
    className = "",
    children,
}: {
    tone?: BadgeTone;
    className?: string;
    children: React.ReactNode;
}) {
    return (
        <span
            className={`inline-flex items-center rounded-full px-2 py-1 text-xs font-medium ${TONE_CLASS[tone]} ${className}`}
        >
            {children}
        </span>
    );
}
