import { useEffect, useRef, type ReactNode } from "react";
import { AlertIcon, CheckIcon, InfoIcon } from "./icons";

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
 * reader reads the message before the user tabs on to the actions. `done`
 * marks a status that finishes something, such as a completed session.
 */
export function Message({
  focus,
  tone,
  done = false,
  text,
  children,
}: {
  focus: boolean;
  tone: "status" | "alert";
  done?: boolean;
  text: ReactNode;
  children?: ReactNode;
}) {
  const ref = useFocusOnMount<HTMLParagraphElement>(focus);
  const iconClass =
    tone === "alert"
      ? "message-icon message-icon-alert"
      : done
        ? "message-icon message-icon-done"
        : "message-icon";
  return (
    <div className="message">
      {/* Decorative: the text says the same. */}
      <span className={iconClass} aria-hidden="true">
        {tone === "alert" ? <AlertIcon /> : done ? <CheckIcon /> : <InfoIcon />}
      </span>
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
