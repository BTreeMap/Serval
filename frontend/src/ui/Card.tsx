/** A surface container with a subtle border, soft elevation and themed
 *  background. The base padding can be overridden via `className`. */
export function Card({
    className = "",
    ...props
}: React.HTMLAttributes<HTMLDivElement>) {
    return (
        <div
            className={`rounded-2xl border border-line bg-surface p-6 shadow-card ${className}`}
            {...props}
        />
    );
}
