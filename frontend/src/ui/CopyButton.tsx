import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "./Button";
import { Check, Copy } from "./icons";

/** Where a {@link CopyButton} gets its bytes: either an eager string known at
 *  render time, or a lazy loader resolved on click. The union makes the two
 *  mutually exclusive — a button is provably one or the other, never both. */
type CopySource = { value: string } | { load: () => Promise<string> };

/** The button's lifecycle. Two independent booleans (`copied`, `loading`) had a
 *  fourth combination — loading *and* showing "Copied!" — that no path could
 *  reach; three named states say the same thing without the dead corner. */
type CopyState = { readonly tag: "idle" } | { readonly tag: "loading" } | { readonly tag: "copied" };

const IDLE: CopyState = Object.freeze({ tag: "idle" as const });
const LOADING: CopyState = Object.freeze({ tag: "loading" as const });
const COPIED: CopyState = Object.freeze({ tag: "copied" as const });

/** How long the "Copied!" confirmation stays up. */
const CONFIRM_MS = 1500;

/** A button that copies text to the clipboard and confirms briefly with an
 *  icon swap. Eager sources copy instantly; lazy sources fetch on click,
 *  showing a spinner while the loader is in flight. */
export function CopyButton(
    props: CopySource & {
        label?: string;
        size?: "sm" | "md";
    },
) {
    const { label = "Copy", size = "md" } = props;
    const [state, setState] = useState<CopyState>(IDLE);

    // The confirmation timer is a resource with a lifetime, so it gets an owner
    // and a disposal path: re-copying replaces it rather than stacking a second
    // timer that would clear the new confirmation early, and unmounting cancels
    // it instead of leaving a pending write to a component that is gone.
    const timer = useRef<number | null>(null);
    const clearTimer = useCallback(() => {
        if (timer.current !== null) {
            window.clearTimeout(timer.current);
            timer.current = null;
        }
    }, []);
    useEffect(() => clearTimer, [clearTimer]);

    const copy = async () => {
        // A load already in flight; ignore re-clicks until it settles.
        if (state.tag === "loading") {
            return;
        }
        try {
            let text: string;
            if ("value" in props) {
                text = props.value;
            } else {
                setState(LOADING);
                text = await props.load();
            }
            await navigator.clipboard.writeText(text);
            setState(COPIED);
            clearTimer();
            timer.current = window.setTimeout(() => {
                timer.current = null;
                setState(IDLE);
            }, CONFIRM_MS);
        } catch {
            setState(IDLE);
        }
    };

    const copied = state.tag === "copied";
    return (
        <Button
            variant="secondary"
            size={size}
            loading={state.tag === "loading"}
            onClick={() => void copy()}
            type="button"
        >
            {copied ? (
                <Check className="h-4 w-4 text-success" aria-hidden />
            ) : (
                <Copy className="h-4 w-4" aria-hidden />
            )}
            {copied ? "Copied!" : label}
        </Button>
    );
}
