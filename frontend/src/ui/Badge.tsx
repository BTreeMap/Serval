/** A small status pill, e.g. a version marker or role indicator. */
export function Badge({
    tone = "neutral",
    className = "",
    children,
}: {
    tone?: "neutral" | "cream" | "wisteria";
    className?: string;
    children: React.ReactNode;
}) {
    const tones: Record<string, string> = {
        neutral: "bg-canvas text-ink-soft ring-1 ring-line",
        cream: "bg-cream/60 text-ink ring-1 ring-cream",
        wisteria: "bg-wisteria/15 text-wisteria-deep ring-1 ring-wisteria/30",
    };
    return (
        <span
            className={`inline-flex items-center rounded-full px-2 py-1 text-xs font-medium ${tones[tone]} ${className}`}
        >
            {children}
        </span>
    );
}
