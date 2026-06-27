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
        "inline-flex items-center justify-center gap-2 rounded-lg px-4 py-2 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-sky-400/60";
    const variants: Record<string, string> = {
        primary: "bg-sky-500 text-white hover:bg-sky-400",
        secondary: "bg-slate-700 text-slate-100 hover:bg-slate-600",
        ghost: "bg-transparent text-slate-300 hover:bg-slate-800",
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
            className={`rounded-xl border border-slate-800 bg-slate-900/60 p-6 shadow-sm ${className}`}
            {...props}
        />
    );
}

/** A small status pill, e.g. immutable vs mutable. */
export function Badge({
    tone = "neutral",
    children,
}: {
    tone?: "neutral" | "amber" | "sky";
    children: React.ReactNode;
}) {
    const tones: Record<string, string> = {
        neutral: "bg-slate-800 text-slate-300",
        amber: "bg-amber-500/15 text-amber-300 ring-1 ring-amber-500/30",
        sky: "bg-sky-500/15 text-sky-300 ring-1 ring-sky-500/30",
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
        <div className="rounded-lg border border-red-500/30 bg-red-500/10 px-4 py-3 text-sm text-red-300">
            {message}
        </div>
    );
}
