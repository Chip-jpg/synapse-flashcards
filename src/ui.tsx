import { useEffect, useRef, type ReactNode } from "react";

/** A stored ISO-8601 time as a short local date and time, e.g. "Sep 16, 2026, 12:50 PM". */
export function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** Focuses the returned element once, when it mounts, if `enabled`. */
export function useFocusOnMount<T extends HTMLElement>(enabled: boolean) {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (enabled) ref.current?.focus();
  }, [enabled]);
  return ref;
}

/**
 * A centred status or error message with optional actions (`children`).
 * When `focus` is set, focus moves to the message text itself, so a screen
 * reader reads the message before the user tabs on to the actions.
 */
export function Message({
  focus,
  tone,
  text,
  children,
}: {
  focus: boolean;
  tone: "status" | "alert";
  text: ReactNode;
  children?: ReactNode;
}) {
  const ref = useFocusOnMount<HTMLParagraphElement>(focus);
  return (
    <div className="message">
      <p
        ref={ref}
        className={tone === "alert" ? "message-error" : undefined}
        role={tone}
        tabIndex={-1}
      >
        {text}
      </p>
      {children}
    </div>
  );
}
