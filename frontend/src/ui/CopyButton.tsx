import { useState } from "react";
import { Button } from "./Button";
import { Check, Copy } from "./icons";

/** A button that copies text to the clipboard and confirms briefly with an
 *  icon swap. */
export function CopyButton({
    value,
    label = "Copy",
    size = "md",
}: {
    value: string;
    label?: string;
    size?: "sm" | "md";
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
        <Button
            variant="secondary"
            size={size}
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
