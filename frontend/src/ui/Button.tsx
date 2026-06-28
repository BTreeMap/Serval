import { LoaderCircle } from "./icons";

/** Action button with consistent styling across the app. Variants cover the
 *  primary action, secondary/neutral actions, a low-emphasis ghost, a
 *  destructive `danger`, and an inline `link` trigger for text contexts.
 *  `loading` shows an inline spinner and disables the button so an in-flight
 *  action cannot be triggered twice. */
export function Button({
    variant = "primary",
    size = "md",
    loading = false,
    className = "",
    disabled,
    children,
    ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
    variant?: "primary" | "secondary" | "ghost" | "danger" | "link";
    size?: "sm" | "md";
    loading?: boolean;
}) {
    const base =
        "inline-flex items-center justify-center gap-2 border font-medium leading-none transition-colors disabled:cursor-not-allowed disabled:opacity-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50 focus-visible:ring-offset-2 focus-visible:ring-offset-surface";
    // link is a dimensionless inline trigger — it inherits text scale from context.
    const sizes: Record<string, string> = {
        sm: "min-h-8 whitespace-nowrap px-3 py-1.5 text-xs",
        md: "min-h-9 whitespace-nowrap px-4 py-2 text-sm",
    };
    const variants: Record<string, string> = {
        primary: "rounded-lg border-transparent bg-wisteria-deep text-white hover:bg-wisteria",
        secondary: "rounded-lg border-line bg-surface text-ink hover:bg-canvas",
        ghost: "rounded-lg border-transparent bg-transparent text-ink-soft hover:bg-canvas hover:text-ink",
        danger: "rounded-lg border-clay/40 bg-clay/10 text-clay hover:bg-clay/20",
        link: "rounded border-transparent bg-transparent text-xs text-ink-soft underline decoration-dotted underline-offset-2 hover:text-wisteria-deep",
    };
    const sizeClass = variant === "link" ? "" : sizes[size];
    return (
        <button
            className={`${base} ${sizeClass} ${variants[variant]} ${className}`}
            disabled={disabled ?? loading}
            aria-busy={loading || undefined}
            {...props}
        >
            {loading && <LoaderCircle className="h-4 w-4 animate-spin" aria-hidden />}
            {children}
        </button>
    );
}
