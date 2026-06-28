import { LoaderCircle } from "./icons";

/** Action button with consistent styling across the app. Variants cover the
 *  primary action, secondary/neutral actions, a low-emphasis ghost, and a
 *  destructive `danger`. `loading` shows an inline spinner and disables the
 *  button so an in-flight action cannot be triggered twice. */
export function Button({
    variant = "primary",
    size = "md",
    loading = false,
    className = "",
    disabled,
    children,
    ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
    variant?: "primary" | "secondary" | "ghost" | "danger";
    size?: "sm" | "md";
    loading?: boolean;
}) {
    const base =
        "inline-flex items-center justify-center gap-2 rounded-lg font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50 focus-visible:ring-offset-2 focus-visible:ring-offset-surface";
    const sizes: Record<string, string> = {
        sm: "px-3 py-1.5 text-xs",
        md: "px-4 py-2 text-sm",
    };
    const variants: Record<string, string> = {
        primary: "bg-wisteria-deep text-white hover:bg-wisteria",
        secondary: "border border-line bg-surface text-ink hover:bg-canvas",
        ghost: "bg-transparent text-ink-soft hover:bg-canvas hover:text-ink",
        danger: "border border-clay/40 bg-clay/10 text-clay hover:bg-clay/20",
    };
    return (
        <button
            className={`${base} ${sizes[size]} ${variants[variant]} ${className}`}
            disabled={disabled ?? loading}
            aria-busy={loading || undefined}
            {...props}
        >
            {loading && <LoaderCircle className="h-4 w-4 animate-spin" aria-hidden />}
            {children}
        </button>
    );
}
