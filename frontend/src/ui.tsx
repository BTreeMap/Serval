import { useEffect, useId, useRef, useState } from "react";

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

/** A text input with a filterable suggestion dropdown. Arbitrary free text is
 *  always allowed; the suggestions are a convenience, not a constraint. */
export function Combobox({
    value,
    onChange,
    options,
    placeholder,
    className = "",
}: {
    value: string;
    onChange: (value: string) => void;
    options: readonly string[];
    placeholder?: string;
    className?: string;
}) {
    const [open, setOpen] = useState(false);
    const [active, setActive] = useState(-1);
    const containerRef = useRef<HTMLDivElement>(null);
    const listId = useId();

    const needle = value.trim().toLowerCase();
    const matches = options.filter((option) =>
        needle === "" ? true : option.toLowerCase().includes(needle),
    );

    useEffect(() => {
        if (!open) {
            return;
        }
        const onPointerDown = (event: PointerEvent) => {
            if (!containerRef.current?.contains(event.target as Node)) {
                setOpen(false);
            }
        };
        document.addEventListener("pointerdown", onPointerDown);
        return () => document.removeEventListener("pointerdown", onPointerDown);
    }, [open]);

    const choose = (option: string) => {
        onChange(option);
        setOpen(false);
        setActive(-1);
    };

    const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
        if (event.key === "ArrowDown") {
            event.preventDefault();
            setOpen(true);
            setActive((prev) => Math.min(prev + 1, matches.length - 1));
        } else if (event.key === "ArrowUp") {
            event.preventDefault();
            setActive((prev) => Math.max(prev - 1, 0));
        } else if (event.key === "Enter") {
            if (open && active >= 0 && active < matches.length) {
                event.preventDefault();
                choose(matches[active]);
            }
        } else if (event.key === "Escape") {
            setOpen(false);
            setActive(-1);
        }
    };

    return (
        <div ref={containerRef} className={`relative ${className}`}>
            <input
                type="text"
                role="combobox"
                aria-expanded={open}
                aria-controls={listId}
                aria-autocomplete="list"
                value={value}
                onChange={(e) => {
                    onChange(e.target.value);
                    setOpen(true);
                    setActive(-1);
                }}
                onFocus={() => setOpen(true)}
                onKeyDown={onKeyDown}
                placeholder={placeholder}
                className="w-full rounded-lg border border-line bg-canvas px-3 py-2 text-sm text-ink focus:border-wisteria focus:outline-none"
            />
            {open && matches.length > 0 && (
                <ul
                    id={listId}
                    role="listbox"
                    className="absolute z-10 mt-1 max-h-60 w-full overflow-auto rounded-lg border border-line bg-surface py-1 shadow-md shadow-ink/10"
                >
                    {matches.map((option, index) => (
                        <li
                            key={option}
                            role="option"
                            aria-selected={index === active}
                            onMouseEnter={() => setActive(index)}
                            onPointerDown={(e) => {
                                e.preventDefault();
                                choose(option);
                            }}
                            className={`cursor-pointer px-3 py-2 text-sm ${
                                index === active
                                    ? "bg-canvas text-ink"
                                    : "text-ink-soft"
                            }`}
                        >
                            {option}
                        </li>
                    ))}
                </ul>
            )}
        </div>
    );
}
