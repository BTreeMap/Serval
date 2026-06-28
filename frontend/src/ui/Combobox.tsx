import { useEffect, useId, useRef, useState } from "react";
import { ChevronDown } from "./icons";
import { controlClass } from "./form";

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
                className={`${controlClass} pr-9`}
            />
            <ChevronDown
                className={`pointer-events-none absolute right-3 top-1/2 h-4 w-4 -translate-y-1/2 text-ink-faint transition-transform ${
                    open ? "rotate-180" : ""
                }`}
                aria-hidden
            />
            {open && matches.length > 0 && (
                <ul
                    id={listId}
                    role="listbox"
                    className="absolute z-10 mt-1 max-h-60 w-full overflow-auto rounded-lg border border-line bg-surface py-1 shadow-pop"
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
