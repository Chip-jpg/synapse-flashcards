import type { ReactNode } from "react";

/**
 * Small line icons drawn inline, so the app needs no icon font or network
 * request. Every icon is decorative: the text beside it (or the control's own
 * label) says what it means, so each is hidden from screen readers.
 */
function Icon({ children }: { children: ReactNode }) {
  return (
    <svg
      className="icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

/** Three connected nodes: the Synapse mark. */
export function LogoIcon() {
  return (
    <Icon>
      <circle cx="6" cy="7" r="2.5" />
      <circle cx="18" cy="7" r="2.5" />
      <circle cx="12" cy="17.5" r="2.5" />
      <path d="M8.5 7h7M7.3 9.2l3.4 6M16.7 9.2l-3.4 6" />
    </Icon>
  );
}

export function PlusIcon() {
  return (
    <Icon>
      <path d="M12 5v14M5 12h14" />
    </Icon>
  );
}

export function PlayIcon() {
  return (
    <Icon>
      <path d="M8 5.5v13l10.5-6.5z" fill="currentColor" />
    </Icon>
  );
}

export function ArrowLeftIcon() {
  return (
    <Icon>
      <path d="M19 12H5M11 6l-6 6 6 6" />
    </Icon>
  );
}

export function ArrowRightIcon() {
  return (
    <Icon>
      <path d="M5 12h14M13 6l6 6-6 6" />
    </Icon>
  );
}

export function PencilIcon() {
  return (
    <Icon>
      <path d="M4 20h4L18.5 9.5a2.1 2.1 0 0 0-3-3L5 17v3zM14 8l2 2" />
    </Icon>
  );
}

export function TrashIcon() {
  return (
    <Icon>
      <path d="M4 7h16M9 7V4.5h6V7M6.5 7l.9 12.1A2 2 0 0 0 9.4 21h5.2a2 2 0 0 0 2-1.9L17.5 7M10 11v6M14 11v6" />
    </Icon>
  );
}

/** A counter-clockwise arrow: bringing something back. */
export function RestoreIcon() {
  return (
    <Icon>
      <path d="M4 12a8 8 0 1 0 2.6-5.9L4 8.5M4 4v4.5h4.5" />
    </Icon>
  );
}

export function ArchiveIcon() {
  return (
    <Icon>
      <rect x="3" y="4" width="18" height="5" rx="1.2" />
      <path d="M5 9v9.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V9M10 13h4" />
    </Icon>
  );
}

/** An open book: the notes area. */
export function BookIcon() {
  return (
    <Icon>
      <path d="M12 6.5C10.5 5 8.3 4.5 4 4.5v13c4.3 0 6.5.5 8 2 1.5-1.5 3.7-2 8-2v-13c-4.3 0-6.5.5-8 2zM12 6.5v13" />
    </Icon>
  );
}

/** A page with lines: one note. */
export function NoteIcon() {
  return (
    <Icon>
      <path d="M14 3.5H7A1.5 1.5 0 0 0 5.5 5v14A1.5 1.5 0 0 0 7 20.5h10a1.5 1.5 0 0 0 1.5-1.5V8L14 3.5zM14 3.5V8h4.5M9 12.5h6M9 16h4" />
    </Icon>
  );
}

/** Stacked cards: a deck. */
export function DeckIcon() {
  return (
    <Icon>
      <path d="M12 3.5l8.5 4.5-8.5 4.5L3.5 8z" />
      <path d="M3.5 12l8.5 4.5 8.5-4.5M3.5 16l8.5 4.5 8.5-4.5" />
    </Icon>
  );
}

export function DownloadIcon() {
  return (
    <Icon>
      <path d="M12 4v11M7.5 10.5L12 15l4.5-4.5M5 20h14" />
    </Icon>
  );
}

export function UploadIcon() {
  return (
    <Icon>
      <path d="M12 20V9M7.5 13.5L12 9l4.5 4.5M5 4h14" />
    </Icon>
  );
}

/** A cylinder: the database backups are made from. */
export function DatabaseIcon() {
  return (
    <Icon>
      <ellipse cx="12" cy="6" rx="7.5" ry="2.75" />
      <path d="M4.5 6v12c0 1.5 3.4 2.75 7.5 2.75s7.5-1.25 7.5-2.75V6M4.5 12c0 1.5 3.4 2.75 7.5 2.75s7.5-1.25 7.5-2.75" />
    </Icon>
  );
}

export function CheckIcon() {
  return (
    <Icon>
      <path d="M5 12.5l4.5 4.5L19 7.5" />
    </Icon>
  );
}

export function AlertIcon() {
  return (
    <Icon>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7.5v5.5M12 16.5v.01" />
    </Icon>
  );
}

export function InfoIcon() {
  return (
    <Icon>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5.5M12 7.5v.01" />
    </Icon>
  );
}
