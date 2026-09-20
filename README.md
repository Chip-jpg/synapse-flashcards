# Synapse

A local-first desktop flashcard app. Everything lives in one SQLite file on your own machine: no
account, no server, no network calls. Reviews are scheduled with [FSRS](https://github.com/open-spaced-repetition/fsrs-rs)
using its default parameters.

Tauri 2 + React + TypeScript on the front, Rust + `sqlx` on the back. Rust owns the database,
migrations, validation, scheduling, and every business rule; React only calls typed Tauri commands.

## What Synapse does today

- **Decks** — create your own, rename them, archive ones you're done with, and unarchive them
  again when you want them back. A built-in read-only
  **Sample deck** is there from first launch so there's something to study immediately.
- **Cards** — add front/back cards, edit them (which never disturbs their schedule), and delete them.
- **Study** — start a review session for a deck, reveal the answer, and rate it **Again / Hard /
  Good / Easy**. FSRS computes the next due date, and every rating is appended to `review_logs`.
- **History is kept** — deleting a card, archiving a deck, and deleting a note are all *soft*: the
  rows stay in the database, a card keeps its FSRS state and review log, and a note keeps its text
  and its times. A deleted note can be restored, and an archived deck can be unarchived; a deleted
  card can't be brought back, yet.
- **Export** — save a `.zip` backup of all your study data wherever you choose.
- **Import (restore)** — replace everything in Synapse with a backup made by Export, after Synapse
  has checked the whole file and you've confirmed. It never merges.
- **Notes** — type up your own plain-text study notes, keep them in a local library, open and edit
  them, and delete ones you're done with. A deleted note leaves the library but is kept under
  *Deleted notes*, where *Restore note* brings it back unchanged.
- **Cards from notes** — with a note on screen, write a card yourself and add it to one of your
  decks. The card is an ordinary card; it isn't linked to the note.

Everything works offline. Turning off networking changes nothing.

## Running it in development

You need [Rust](https://www.rust-lang.org/tools/install), [Node.js](https://nodejs.org/), and the
[Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS (on Windows: the
WebView2 runtime and the MSVC build tools).

```sh
npm install
npm run tauri dev
```

That starts Vite and opens Synapse as a native window. The checks the project is kept green against:

```sh
cargo fmt --check                          # in src-tauri/
cargo test                                 # in src-tauri/
cargo clippy --all-targets -- -D warnings  # in src-tauri/
npm run build                              # tsc + vite, from the project root
```

### Using a throwaway database

By default a development build reads and writes your real database, the same one a release build
uses. To keep experiments away from it, set `SYNAPSE_DB_DIR` to any folder — a debug build will put
`synapse.sqlite` there instead:

```sh
# PowerShell
$env:SYNAPSE_DB_DIR = "$PWD\src-tauri\target\scratch-db"; npm run tauri dev
```

```sh
# bash
SYNAPSE_DB_DIR="$PWD/src-tauri/target/scratch-db" npm run tauri dev
```

To reset that scratch database, close the app and delete the folder. **Only ever delete a folder you
pointed `SYNAPSE_DB_DIR` at** — never the real one. This override exists in debug builds only;
release builds always use the OS data directory (`%APPDATA%\synapse` on Windows,
`~/Library/Application Support/synapse` on macOS, `~/.local/share/synapse` on Linux).

Synapse never deletes or resets its own database; there is no in-app "reset everything". The only
thing that replaces it is *Import data*, and only after you confirm.

## Using it

**Make a deck and some cards.** On the dashboard, *Create deck* → name it (a description is
optional) → *Open deck* → *Add card* → fill in front and back. Names must be unique ignoring case;
line breaks inside a card are kept as you typed them.

**Study.** Press *Start review* on any deck with cards due. Reveal the answer, then rate it. The
session ends when nothing in that deck is due. Closing the app mid-session is safe — reopening the
deck resumes it.

**Edit or delete a card.** On the deck screen, *Edit card* changes the text and keeps the card's
schedule and history. *Delete card* asks for confirmation, then removes the card from lists, counts,
and reviews. The card and its review log stay in the database, and there's no way to bring it back.

**Rename or archive a deck.** Also on the deck screen. Archiving asks for confirmation, then moves
the deck to *Archived decks* on the dashboard: it can't be reviewed, changed, or given new cards, and
any unfinished session in it is closed. Its cards and history are kept, and its name stays taken.

**Unarchive a deck.** Under *Archived decks* on the dashboard, *Unarchive deck* asks for
confirmation, then puts the deck back in your deck list exactly as it was: same name, description,
cards, FSRS state, review log, and sessions. Its cards become due again only on the dates they
already had, no review session starts, and cards you deleted before archiving stay deleted.

**Export your data.** On the dashboard, *Your data* → *Export data*. Pick where to save the `.zip`.
It contains exactly two files: `manifest.json` (format, format version, schema version, app version,
and export time) and `db/synapse.sqlite`, a complete snapshot of your database.

**Restore a backup.** *Your data* → *Import data* → pick a `.zip` made by *Export data*. Synapse
checks the whole file first — only its own export format is accepted, and anything else is refused
with nothing changed. If the backup is sound, Synapse names it and asks whether to replace **all**
your current decks, cards, reviews, sessions, and notes with the backup's. *Keep current data*
changes nothing (export first if you want a copy). *Replace my data* swaps the database in one step,
keeping a rollback copy until the restored database has opened and passed its checks; the dashboard
then reloads from the backup. A backup from an older version of Synapse is upgraded as it's
restored; one from a newer version is refused.

**Write notes.** On the dashboard, *Notes* → *Open notes* → *New note*. A note has a title (up to
200 characters) and a plain-text body (up to 100,000 characters); both are required. Line breaks
and indentation are kept, and only blank space at the very start and end is trimmed. *Open note*
shows it, *Edit note* changes it. The library lists the most recently edited note first.

**Delete or restore a note.** Open a note → *Delete note* asks for confirmation, then takes the note
out of your library: it can't be opened, edited, or used to write a card. Nothing is lost — the note
appears under *Deleted notes* at the bottom of the library, with when it was deleted, and *Restore
note* puts it back exactly as it was, in the same place in the list. Deleting a note never touches
cards you wrote from it, and a note is never removed from the database.

**Write a card from a note.** Open a note → *Create card from note*. The note stays visible while you
choose one of your own active decks and type the front and back yourself; nothing is filled in for
you. The card is checked exactly like one added on the deck screen, is due straight away, and isn't
linked to the note — editing the note later never changes the card. After saving, *Open <deck>* takes
you to the deck the card went into.

The Sample deck is read-only: you can study it, but not rename, archive, edit it, or add cards to it.

## Current limitations

- **Import replaces; it never merges.** Restoring a backup replaces all current data, and that can't
  be undone from the app. Only Synapse's own exports are accepted (no Anki files, CSV, or raw
  `.sqlite` files), a backup whose database unpacks to more than 1 GiB is refused, and there's no
  preview of a backup's contents beyond its file name and export time. A restore that's interrupted
  (the process killed mid-swap) can leave a rollback copy in Synapse's data folder
  (`import-workspace/rollback.sqlite`, later renamed `rollback-kept-<time>.sqlite`); these are never
  deleted automatically.
- **No card undelete.** A deleted card is kept in the database with its review log, but can't be
  brought back from the app. Decks and notes can be brought back; cards can't.
- **Notes are plain typed text.** No formatting, attachments, PDFs, OCR, audio, web clips, search,
  tags, or folders. Nothing generates cards from a note, and a card doesn't remember the note it was
  written from — so deleting a note leaves its cards exactly where they are. A deleted note is
  listed by title and dates only; there's no way to read its text again without restoring it.
- **The dashboard is the way in.** *Notes*, *Export data*, and *Import data* sit below the deck list,
  so if the deck list can't load they aren't reachable until *Retry* succeeds.
- **No sync, accounts, cloud, or collaboration.** By design.
- **Default FSRS parameters only.** The optimizer that re-fits weights to your own history is
  future work and isn't built.
- **One card type.** Front/back text only: no images, audio, cloze, or tags.
- **No search, bulk actions, folders, or statistics.**
- **No automated frontend tests.** The Rust core has unit tests (`cargo test`); React is checked by
  TypeScript and manual end-to-end testing.

## Architecture

- **`src/`** — the React UI. `api.ts` is the only file that calls Rust, through typed Tauri
  commands. The screens are `Dashboard.tsx`, `DeckDetail.tsx`, `ReviewSession.tsx`, and `Notes.tsx`.
- **`src-tauri/src/`** — the Rust core:
  - `lib.rs` — app setup and command registration; `main.rs` is a thin entry point.
  - `commands.rs` — the Tauri commands, the only way the frontend reaches the core.
  - `db.rs` — where the database lives, opening it, running migrations, and the one-time seed.
  - `scheduler.rs` — FSRS scheduling via the `fsrs` crate, with no database access.
  - `study.rs` — the deck dashboard, review sessions, and recording reviews.
  - `authoring.rs` — creating, renaming, archiving, and unarchiving decks; adding, editing, and
    deleting cards.
  - `notes.rs` — typed notes: writing, editing, soft-deleting, and restoring them.
  - `export.rs` / `import.rs` — writing a `.zip` backup, and checking and restoring one.
- **`src-tauri/migrations/`** — forward-only `sqlx` migrations (`0001`–`0009`), embedded in the
  binary at compile time.
