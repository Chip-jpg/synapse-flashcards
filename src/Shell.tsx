import { Fragment, type ReactNode } from "react";
import { useT, type MessageKey } from "./i18n";
import {
  ArchiveIcon,
  BookIcon,
  CalendarIcon,
  ChartIcon,
  DatabaseIcon,
  DeskIcon,
  FileIcon,
  LogoIcon,
  SearchIcon,
  SlidersIcon,
  TagIcon,
} from "./icons";

/** The places the sidebar leads to. */
export type Place =
  | "desk"
  | "search"
  | "tags"
  | "exams"
  | "notes"
  | "sources"
  | "stats"
  | "archived"
  | "backup"
  | "settings";

const PLACES: { place: Place; label: MessageKey; icon: ReactNode }[] = [
  { place: "desk", label: "nav.desk", icon: <DeskIcon /> },
  { place: "search", label: "nav.search", icon: <SearchIcon /> },
  { place: "tags", label: "nav.tags", icon: <TagIcon /> },
  { place: "exams", label: "nav.exams", icon: <CalendarIcon /> },
  { place: "notes", label: "nav.notes", icon: <BookIcon /> },
  { place: "sources", label: "nav.sources", icon: <FileIcon /> },
  { place: "stats", label: "nav.stats", icon: <ChartIcon /> },
  { place: "archived", label: "nav.archived", icon: <ArchiveIcon /> },
  { place: "backup", label: "nav.backup", icon: <DatabaseIcon /> },
  { place: "settings", label: "nav.settings", icon: <SlidersIcon /> },
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
  const { t } = useT();
  return (
    <div className="app">
      <header className="sidebar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">
            <LogoIcon />
          </span>
          <div>
            <h1 className="app-title">Synapse</h1>
            <p className="brand-tagline">{t("common.tagline")}</p>
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
                  {t(item.label)}
                </button>
              </li>
            ))}
          </ul>
        </nav>

        <div className="sidebar-note">
          <p className="sidebar-note-title">
            <span className="dot" aria-hidden="true" />
            {t("common.localDatabase")}
          </p>
          <p>{t("common.localDescription")}</p>
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
            {t("common.stored")}
          </p>
        </div>

        <div className="stage">{children}</div>
      </main>
    </div>
  );
}
