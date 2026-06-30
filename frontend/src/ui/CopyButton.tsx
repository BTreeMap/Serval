import { useState } from "react";
import { Button } from "./Button";
import { Check, Copy } from "./icons";

/** Where a {@link CopyButton} gets its bytes: either an eager string known at
 *  render time, or a lazy loader resolved on click. The union makes the two
 *  mutually exclusive — a button is provably one or the other, never both. */
type CopySource = { value: string } | { load: () => Promise<string> };

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
    const [copied, setCopied] = useState(false);
    const [loading, setLoading] = useState(false);

    const copy = async () => {
        // A load already in flight; ignore re-clicks until it settles.
        if (loading) {
            return;
        }
        try {
            let text: string;
            if ("value" in props) {
                text = props.value;
            } else {
                setLoading(true);
                text = await props.load();
            }
            await navigator.clipboard.writeText(text);
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
        } catch {
            setCopied(false);
        } finally {
            setLoading(false);
        }
    };

    return (
        <Button
            variant="secondary"
            size={size}
            loading={loading}
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
