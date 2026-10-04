import { Fragment, type ReactNode } from "react";
import { ArchiveIcon, BookIcon, DatabaseIcon, DeskIcon, LogoIcon } from "./icons";

/** The places the sidebar leads to. */
export type Place = "desk" | "notes" | "archived" | "backup";

const PLACES: { place: Place; label: string; icon: ReactNode }[] = [
  { place: "desk", label: "Study Desk", icon: <DeskIcon /> },
  { place: "notes", label: "Notes", icon: <BookIcon /> },
  { place: "archived", label: "Archived decks", icon: <ArchiveIcon /> },
  { place: "backup", label: "Backup & restore", icon: <DatabaseIcon /> },
];

/**
 * The frame around every screen: a sidebar with the app's places, and a bar
 * over the screen that says where in the app it is.
 */
export function Shell({
  place,
  atPlace,
  trail,
  locked,
  onNavigate,
  children,
}: {
  /** The place the current screen belongs to, marked in the sidebar. */
  place: Place;
  /** Whether the screen is that place itself, rather than one opened from it. */
  atPlace: boolean;
  /** Where the screen is, outermost first. */
  trail: string[];
  /** A backup is being checked or restored, so the sidebar can't be used. */
  locked: boolean;
  /** A sidebar place was chosen. */
  onNavigate: (place: Place) => void;
  children: ReactNode;
}) {
  return (
    <div className="app">
      <header className="sidebar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">
            <LogoIcon />
          </span>
          <div>
            <h1 className="app-title">Synapse</h1>
            <p className="brand-tagline">Flashcards and notes</p>
          </div>
        </div>

        <nav aria-label="Synapse" inert={locked}>
          <ul className="nav-list">
            {PLACES.map((item) => (
              <li key={item.place}>
                <button
                  type="button"
                  className="nav-item"
                  aria-current={item.place === place ? (atPlace ? "page" : "true") : undefined}
                  onClick={() => onNavigate(item.place)}
                >
                  {item.icon}
                  {item.label}
                </button>
              </li>
            ))}
          </ul>
        </nav>

        <div className="sidebar-note">
          <p className="sidebar-note-title">
            <span className="dot" aria-hidden="true" />
            Local database
          </p>
          <p>Your decks, cards, reviews, and notes are saved on this computer.</p>
        </div>
      </header>

      {/* The bar is inside `main`, so where you are is read with the screen. */}
      <main className="workspace">
        <div className="workspace-bar">
          <p className="crumbs">
            {trail.map((step, index) => (
              <Fragment key={step}>
                {index > 0 && (
                  <span className="crumb-separator" aria-hidden="true">
                    /
                  </span>
                )}
                <span className={index === trail.length - 1 ? "crumb-current" : undefined}>
                  {step}
                </span>
              </Fragment>
            ))}
          </p>
          <p className="local-badge">
            <span className="dot" aria-hidden="true" />
            Stored on this computer
          </p>
        </div>

        <div className="stage">{children}</div>
      </main>
    </div>
  );
}
