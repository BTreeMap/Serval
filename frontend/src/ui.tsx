import { useState } from "react";

/** A primary/secondary/danger action button with consistent styling. */
export function Button({
    variant = "primary",
    className = "",
    ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
    variant?: "primary" | "secondary" | "ghost";
}) {
    const base =
        "inline-flex items-center justify-center gap-2 rounded-lg px-4 py-2 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/50";
    const variants: Record<string, string> = {
        primary: "bg-wisteria-deep text-white hover:bg-wisteria",
        secondary: "border border-line bg-surface text-ink hover:bg-canvas",
        ghost: "bg-transparent text-ink-soft hover:bg-canvas hover:text-ink",
    };
    return (
        <button className={`${base} ${variants[variant]} ${className}`} {...props} />
    );
}

/** A surface container with subtle border and background. */
export function Card({
    className = "",
    ...props
}: React.HTMLAttributes<HTMLDivElement>) {
    return (
        <div
            className={`rounded-2xl border border-line bg-surface p-6 shadow-sm shadow-ink/5 ${className}`}
            {...props}
        />
    );
}

/** A small status pill, e.g. a version marker. */
export function Badge({
    tone = "neutral",
    children,
}: {
    tone?: "neutral" | "cream" | "wisteria";
    children: React.ReactNode;
}) {
    const tones: Record<string, string> = {
        neutral: "bg-canvas text-ink-soft ring-1 ring-line",
        cream: "bg-cream/60 text-ink ring-1 ring-cream",
        wisteria: "bg-wisteria/15 text-wisteria-deep ring-1 ring-wisteria/30",
    };
    return (
        <span
            className={`inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium ${tones[tone]}`}
        >
            {children}
        </span>
    );
}

/** A button that copies text to the clipboard and confirms briefly. */
export function CopyButton({
    value,
    label = "Copy",
}: {
    value: string;
    label?: string;
}) {
    const [copied, setCopied] = useState(false);

    const copy = async () => {
        try {
            await navigator.clipboard.writeText(value);
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
        } catch {
            setCopied(false);
        }
    };

    return (
        <Button variant="secondary" onClick={() => void copy()} type="button">
            {copied ? "Copied!" : label}
        </Button>
    );
}

/** An inline error banner. */
export function ErrorBanner({ message }: { message: string }) {
    return (
        <div className="rounded-lg border border-clay/30 bg-clay/10 px-4 py-3 text-sm text-clay">
            {message}
        </div>
    );
}
