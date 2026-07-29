import { useCallback, useState } from "react";
import { messageOf } from "./errors";

/** The states an inline "click to edit, explicit save" control can be in.
 *
 *  It replaces four independent cells — `editing`, `draft`, `busy`, `error` —
 *  whose sixteen combinations included a saving control that is not editing, an
 *  error shown while not editing, and an error shown while a save is in flight.
 *  A draft exists in exactly the three states that have one, so no branch has to
 *  ask what `draft` means while `editing` is false. */
export type InlineEditState =
    | { readonly tag: "viewing" }
    | { readonly tag: "editing"; readonly draft: string }
    | { readonly tag: "saving"; readonly draft: string }
    | { readonly tag: "failed"; readonly draft: string; readonly message: string };

const VIEWING: InlineEditState = Object.freeze({ tag: "viewing" as const });

export interface InlineEdit {
    readonly state: InlineEditState;
    /** True in every state that shows a control rather than the display form. */
    readonly isOpen: boolean;
    /** True while a commit is in flight — the one reason to disable Cancel. */
    readonly isSaving: boolean;
    /** The text in the box, or `""` while not editing. */
    readonly draft: string;
    /** The message to surface, or `null`. */
    readonly message: string | null;
    readonly begin: () => void;
    readonly cancel: () => void;
    readonly setDraft: (draft: string) => void;
    readonly save: () => void;
}

/** The shared state machine behind every inline editor in the app.
 *
 *  Trimming happens here, once, so `isUnchanged` and `commit` both see the same
 *  normalised text and cannot disagree about whether a save is a no-op. The
 *  untrimmed draft stays in the box, matching what the user typed. */
export function useInlineEdit(params: {
    /** The text to seed the box with when editing begins. */
    readonly initial: () => string;
    /** Given the trimmed draft, is committing it a no-op? A no-op closes the
     *  editor without a request — the two call sites disagree on what counts
     *  (a description may be cleared to empty; a content type may not), so the
     *  predicate is the caller's to state. */
    readonly isUnchanged: (trimmed: string) => boolean;
    /** Persist the trimmed draft. Rejecting leaves the editor open with the
     *  draft intact so the user can retry without retyping. */
    readonly commit: (trimmed: string) => Promise<void>;
}): InlineEdit {
    const { initial, isUnchanged, commit } = params;
    const [state, setState] = useState<InlineEditState>(VIEWING);

    const begin = useCallback(() => {
        setState({ tag: "editing", draft: initial() });
    }, [initial]);

    const cancel = useCallback(() => setState(VIEWING), []);

    const setDraft = useCallback((draft: string) => {
        // Typing keeps the current state's meaning: a failure stays visible
        // until the next save or cancel, as it did before.
        setState((prev) =>
            prev.tag === "viewing" || prev.tag === "saving" ? prev : { ...prev, draft },
        );
    }, []);

    // Reads `state` from the closure rather than from a `setState` updater: an
    // updater must be pure, and React invokes it twice under StrictMode, which
    // would fire the request twice.
    const save = useCallback(() => {
        if (state.tag === "viewing" || state.tag === "saving") {
            return;
        }
        const { draft } = state;
        const trimmed = draft.trim();
        if (isUnchanged(trimmed)) {
            setState(VIEWING);
            return;
        }
        setState({ tag: "saving", draft });
        void commit(trimmed).then(
            () => setState(VIEWING),
            (error: unknown) =>
                setState({ tag: "failed", draft, message: messageOf(error) }),
        );
    }, [state, isUnchanged, commit]);

    return {
        state,
        isOpen: state.tag !== "viewing",
        isSaving: state.tag === "saving",
        draft: state.tag === "viewing" ? "" : state.draft,
        message: state.tag === "failed" ? state.message : null,
        begin,
        cancel,
        setDraft,
        save,
    };
}
