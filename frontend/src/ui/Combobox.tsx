import { useEffect, useId, useMemo, useRef, useState } from "react";
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
    // The highlighted suggestion is identified by its *value*, not by an index
    // into a list that re-filters as the user types. An index and its list are
    // two things that can disagree — a `-1` sentinel plus a bound that shifts
    // under it — whereas a value is either still among the matches or is not.
    const [activeOption, setActiveOption] = useState<string | null>(null);
    const containerRef = useRef<HTMLDivElement>(null);
    const listId = useId();

    const needle = value.trim().toLowerCase();
    const matches = useMemo(
        () => (needle === "" ? options : options.filter((o) => o.toLowerCase().includes(needle))),
        [options, needle],
    );

    // -1 when nothing is highlighted, or when the highlight fell out of the
    // current matches. Both read the same way to the caller: "no cursor yet".
    const activeIndex = activeOption === null ? -1 : matches.indexOf(activeOption);

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
        setActiveOption(null);
    };

    /** Move the highlight by `step`, clamped to the ends of the match list. */
    const move = (step: number) => {
        if (matches.length === 0) {
            setActiveOption(null);
            return;
        }
        const next = Math.min(Math.max(activeIndex + step, 0), matches.length - 1);
        setActiveOption(matches[next]);
    };

    const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
        if (event.key === "ArrowDown") {
            event.preventDefault();
            setOpen(true);
            move(1);
        } else if (event.key === "ArrowUp") {
            event.preventDefault();
            move(-1);
        } else if (event.key === "Enter") {
            if (open && activeIndex >= 0) {
                event.preventDefault();
                choose(matches[activeIndex]);
            }
        } else if (event.key === "Escape") {
            setOpen(false);
            setActiveOption(null);
        }
    };

    const optionId = (index: number) => `${listId}-option-${index}`;
    const showList = open && matches.length > 0;

    return (
        <div ref={containerRef} className={`relative ${className}`}>
            <input
                type="text"
                role="combobox"
                aria-expanded={open}
                aria-controls={listId}
                aria-autocomplete="list"
                // Completes the ARIA contract the listbox already declares:
                // without it, the keyboard highlight below is invisible to a
                // screen reader.
                aria-activedescendant={
                    showList && activeIndex >= 0 ? optionId(activeIndex) : undefined
                }
                value={value}
                onChange={(e) => {
                    onChange(e.target.value);
                    setOpen(true);
                    setActiveOption(null);
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
            {showList && (
                <ul
                    id={listId}
                    role="listbox"
                    className="absolute z-10 mt-1 max-h-64 w-full overflow-auto rounded-lg border border-line bg-surface py-2 shadow-pop"
                >
                    {matches.map((option, index) => (
                        <li
                            key={option}
                            id={optionId(index)}
                            role="option"
                            aria-selected={index === activeIndex}
                            onMouseEnter={() => setActiveOption(option)}
                            onPointerDown={(e) => {
                                e.preventDefault();
                                choose(option);
                            }}
                            className={`cursor-pointer px-3 py-2 text-sm ${
                                index === activeIndex
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
