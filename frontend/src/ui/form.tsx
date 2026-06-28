
/** Shared field-control styling so inputs, textareas and the combobox all share
 *  one focus treatment and themed surface. */
export const controlClass =
    "w-full rounded-lg border border-line bg-canvas px-3 py-2 text-sm text-ink placeholder:text-ink-faint transition-colors focus:border-wisteria focus:outline-none focus-visible:ring-2 focus-visible:ring-wisteria/40";

/** A single-line text input with shared control styling. */
export function Input({
    className = "",
    ...props
}: React.InputHTMLAttributes<HTMLInputElement>) {
    return (
        <input
            className={`${controlClass} ${className}`}
            {...props}
        />
    );
}

/** A multi-line text input with shared control styling. Defaults to vertical
 *  resize and a monospace face suited to code/template content. */
export function Textarea({
    className = "",
    mono = true,
    ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement> & { mono?: boolean }) {
    return (
        <textarea
            className={`${controlClass} resize-y ${mono ? "font-mono" : ""} ${className}`}
            {...props}
        />
    );
}
