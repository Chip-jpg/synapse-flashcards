# Synapse

A local-first desktop flashcard app. Everything lives in one SQLite file on your own machine: no
account, no server, and nothing sent over the internet. Reviews are scheduled with [FSRS](https://github.com/open-spaced-repetition/fsrs-rs)
using its default parameters. If you run [Ollama](https://ollama.com) on the same computer, a model
there can suggest cards from your notes; Synapse talks to nothing else.

Tauri 2 + React + TypeScript on the front, Rust + `sqlx` on the back. Rust owns the database,
migrations, validation, scheduling, and every business rule; React only calls typed Tauri commands.

## What Synapse does today

- **Decks** — create your own, rename them, archive ones you're done with, and unarchive them
  again when you want them back. A built-in read-only
  **Sample deck** is there from first launch so there's something to study immediately.
- **Cards** — add front/back cards, edit them (which never disturbs their schedule), delete them, and
  restore ones you deleted.
- **Study** — start a review session for a deck, reveal the answer, and rate it **Again / Hard /
  Good / Easy**, with **Card 2 of 5** and a progress bar as you go. The whole session can be
  answered from the keyboard: **Space** reveals, **1–4** rate. FSRS computes the next due date, and
  every rating is appended to `review_logs`.
- **Today** — the Study Desk shows how many reviews you've recorded today and for how many
  different cards (*Today: 12 reviews · 9 cards*), counted since midnight on your computer's clock.
- **History is kept** — deleting a card, archiving a deck, and deleting a note are all *soft*: the
  rows stay in the database, a card keeps its FSRS state and review log, and a note keeps its text
  and its times. Every one of them can be brought back: a deleted card restored, an archived deck
  unarchived, a deleted note restored — each exactly as it was.
- **Export** — save a `.zip` backup of all your study data wherever you choose.
- **Import (restore)** — replace everything in Synapse with a backup made by Export, after Synapse
  has checked the whole file and you've confirmed. It never merges.
- **Notes** — type up your own plain-text study notes, or import them from a file, keep them in a
  local library, open and edit them, and delete ones you're done with. A deleted note leaves the library but is kept under
  *Deleted notes*, where *Restore note* brings it back unchanged.
- **Import files as notes** — make a note from the text of a PDF, text (`.txt`), Markdown (`.md`),
  Word (`.docx`), or PowerPoint (`.pptx`) file. You check and edit the text before it's saved; the
  file itself isn't kept.
- **Cards from notes** — with a note on screen, write a card yourself and add it to one of your
  decks. The card is an ordinary card; it isn't linked to the note.
- **Suggested cards (optional)** — have a model in Ollama, on your own computer, suggest up to ten
  cards from a note, each shown with the passage of the note it comes from. You check, edit, and
  remove them, then save the ones you want into a deck. Nothing is saved until you do, and the
  note never leaves your computer.

Everything works offline. Turning off networking changes nothing: suggestions use Ollama on this
computer, and the rest of Synapse doesn't need it at all.

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
npm test                                   # frontend tests (Vitest), from the project root
```

The Rust tests answer for Ollama with a stand-in. One more test, skipped by default, asks a real
Ollama running on this computer, to catch changes in its API:

```sh
# in src-tauri/, with the model installed in Ollama
SYNAPSE_TEST_OLLAMA_MODEL=llama3.2:3b cargo test real_ollama -- --ignored
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

**Find your way around.** The sidebar has four places: the **Study Desk** (your decks, with the
first one that has cards due at the top), **Notes**, **Archived decks**, and **Backup & restore**.
In a narrow window it becomes a bar across the top.

**Unsaved changes are never dropped silently.** If you've typed something in a form (a deck, card,
or note) and haven't saved it, choosing a place in the sidebar or closing the window first asks
whether to discard it. *Keep editing* (or **Escape**) leaves you in the form with your text intact;
*Discard changes* carries on. A form you opened but haven't changed, or changed back to how it
started, doesn't ask. *Cancel* still discards a form's changes without asking, and nothing is kept
as a draft: if you discard, what you typed is gone.

**Make a deck and some cards.** On the Study Desk, *Create deck* → name it (a description is
optional) → *Open deck* → *Add card* → fill in front and back. Names must be unique ignoring case;
line breaks inside a card are kept as you typed them.

**Study.** Press *Start review* on any deck with cards due. Reveal the answer, then rate it. The
session ends when nothing in that deck is due. Closing the app or leaving through the sidebar
mid-session is safe — starting a review of that deck again resumes it.

Above the card, *Card 2 of 5* and a bar show how far you've got. The card you're looking at counts
towards the total, so the bar fills only once you finish the session. The total is read fresh with
each card rather than fixed when the session starts: if another card falls due while you're
studying, the total goes up instead of the bar sliding backwards.

**Study from the keyboard.** **Space** or **Enter** reveals the answer, then **1** = Again,
**2** = Hard, **3** = Good, **4** = Easy. The number keys do nothing until the answer is revealed,
and nothing while a rating is saving, so a stray press can't rate the card twice or rate the next
card by accident. The buttons are always there too — a key and a click do exactly the same thing.

**Edit or delete a card.** On the deck screen, *Edit card* changes the text and keeps the card's
schedule and history. *Delete card* asks for confirmation, then takes the card out of the deck's
lists, counts, and reviews. The card and its review log stay in the database, and it appears under
*Deleted cards* at the bottom of the deck screen.

**Restore a card.** Under *Deleted cards*, *Restore card* asks for confirmation, then puts the card
back in the deck exactly as it was: same text, FSRS state, due date, reps, lapses, and review log.
It is not reset to a new card, so it becomes due again only on the date it already had, and no
review session starts. Cards in an archived deck can't be restored until the deck is unarchived,
and the Sample deck's cards stay out of this entirely.

**Rename or archive a deck.** Also on the deck screen. Archiving asks for confirmation, then moves
the deck to *Archived decks*: it can't be reviewed, changed, or given new cards, and any unfinished
session in it is closed. Its cards and history are kept, and its name stays taken.

**Unarchive a deck.** In *Archived decks*, *Unarchive deck* asks for confirmation, then puts the
deck back on the Study Desk exactly as it was: same name, description,
cards, FSRS state, review log, and sessions. Its cards become due again only on the dates they
already had, no review session starts, and cards you deleted before archiving come back still
deleted — you can restore them from the deck once it's back.

**Export your data.** *Backup & restore* → *Export data*. Pick where to save the `.zip`.
It contains exactly two files: `manifest.json` (format, format version, schema version, app version,
and export time) and `db/synapse.sqlite`, a complete snapshot of your database.

**Restore a backup.** *Backup & restore* → *Import data* → pick a `.zip` made by *Export data*. Synapse
checks the whole file first — only its own export format is accepted, and anything else is refused
with nothing changed. If the backup is sound, Synapse names it and asks whether to replace **all**
your current decks, cards, reviews, sessions, and notes with the backup's. *Keep current data*
changes nothing (export first if you want a copy). *Replace my data* swaps the database in one step,
keeping a rollback copy until the restored database has opened and passed its checks; the Study
Desk then reloads from the backup. A backup from an older version of Synapse is upgraded as it's
restored; one from a newer version is refused. While a backup is being checked, waits for your
answer, or is being restored — and while an export is being saved — the sidebar can't be used, so
nothing else can start in the meantime.

**Write notes.** *Notes* (in the sidebar, or *Open notes* on the Study Desk) → *New note*. A note
has a title (up to 200 characters) and a plain-text body (up to 100,000 characters); both are
required. Line breaks and indentation are kept, and only blank space at the very start and end is
trimmed. *Open note* shows it, *Edit note* changes it. The library lists the most recently edited
note first.

**Import a file as a note.** *Notes* → *Import file* → pick a PDF, text (`.txt`), Markdown (`.md`),
Word (`.docx`), or PowerPoint (`.pptx`) file. Synapse reads its text and opens it in the new-note form,
titled after the document (its own title if it has one, else the file name), with a line saying
what was read. Check the text, edit it as you like, and *Save note*; nothing is saved before that,
and leaving or closing first asks whether to discard it. The note is an ordinary note: Synapse
doesn't keep the file, link to it, or remember where it was.

What's taken from each kind of file:

- **PDF** — the text of each page, in order, with lines that were only broken at the edge of the
  page joined up again. Pages with no text (usually scanned images) are named, so you can check
  nothing's missing; a PDF with no text at all, such as a scan, is refused, since Synapse can't
  read pictures of text. Password-protected PDFs can't be read.
- **Text and Markdown** — the whole file, as written (UTF-8, UTF-16, or older Windows text).
- **Word** — the document's paragraphs, list items as `- ` lines, and tables a row per line. Headers,
  footers, footnotes, comments, and images are left out.
- **PowerPoint** — each slide's text in the order the slides are shown, under *Slide 1*, *Slide 2*,
  and so on, followed by its speaker notes. Images are left out.

Files up to 50 MB can be imported; a note holds up to 100,000 characters, so a longer text has to
be shortened before it's saved (Synapse says so). Older `.doc` and `.ppt` files need saving in the
newer format first. A PDF is read by a second, short-lived run of Synapse, so a damaged file can't
crash the app: it's just reported as a file that couldn't be read. Text over 12,000 characters can
still be saved as one note; *Suggest cards* then offers it in parts (Synapse says so).

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

**Have cards suggested from a note.** This needs [Ollama](https://ollama.com) installed and running
on the same computer, with at least one model downloaded (for example `ollama pull llama3.2`;
larger models write better cards but take longer). Open a note → *Suggest cards* → choose a model →
*Suggest cards*. The model reads the note and suggests up to ten cards; this can take a minute or
more without a fast graphics card, and *Stop* ends it. Then check every card, because models get
things wrong: edit the front or back, *Remove card* for any you don't want, choose a deck, and
*Save*.

Cards are meant to come only from your note, not from what the model already knows. The model is
told so, and must quote, for each card, the passage of your note its answer comes from. Synapse
then checks every card, and leaves out (and tells you about) any whose quote isn't in your note
word for word, or whose answer isn't taken from its quote. Each card you're shown has its passage
under it, *From your note*, and if its answer uses words your note never does, they're listed
(*Not in your note: …*), which often means the model added something. These checks catch most
things a model brings in from outside, but a model can't be made to forget what it knows, so they
can't catch everything: compare each card with its passage before you save it. All the cards are saved together, checked exactly like cards you type, and are due
straight away; if any is refused, none is saved and the one to fix is pointed out. Until you save,
nothing is stored anywhere: leaving or closing first asks whether to discard the suggestions.
Synapse remembers which model you used last.

Only Ollama at its usual address on this computer (`127.0.0.1:11434`) is ever asked, and only
when you press *Suggest cards*. Ollama's cloud models, which run on Ollama's servers, are never
listed or used, so the note stays on your computer.

A model reads up to 12,000 characters at a time, so a longer note, such as an imported document,
is offered in parts of up to 12,000 characters, each ending where a paragraph does. Choose one under
*Part of the note*; the screen shows that part, the model reads only that part, and its cards are
checked against it. Come back to *Suggest cards* for the other parts.

The Sample deck is read-only: you can study it, but not rename, archive, edit it, or add cards to it.

## Current limitations

- **Import replaces; it never merges.** Restoring a backup replaces all current data, and that can't
  be undone from the app. Only Synapse's own exports are accepted (no Anki files, CSV, or raw
  `.sqlite` files), a backup whose database unpacks to more than 1 GiB is refused, and there's no
  preview of a backup's contents beyond its file name and export time. A restore that's interrupted
  (the process killed mid-swap) can leave a rollback copy in Synapse's data folder
  (`import-workspace/rollback.sqlite`, later renamed `rollback-kept-<time>.sqlite`); these are never
  deleted automatically.
- **Restoring is one card at a time.** *Deleted cards* lists a deck's deleted cards and restores
  them one by one; there's no "restore all", no undo of a restore beyond deleting the card again,
  and no way to move a card to a different deck on the way back.
- **Notes are plain text.** No formatting, attachments, audio, web clips, search, tags, or folders.
  Importing a file takes only its text: the file isn't kept, layout and images are lost, and there's
  no OCR for scanned pages. A card doesn't remember the note it was written or suggested from — so
  deleting a note leaves its cards exactly where they are. A deleted note is listed by title and
  dates only; there's no way to read its text again without restoring it.
- **No sync, accounts, cloud, or collaboration.** By design.
- **Default FSRS parameters only.** The optimizer that re-fits weights to your own history is
  future work and isn't built.
- **One card type.** Front/back text only: no images, audio, cloze, or tags.
- **No search, bulk actions, or folders.**
- **Statistics are one daily count.** The Study Desk counts today's reviews and the different
  cards they were for, nothing more: no history of earlier days, no streaks, no percentages. It
  counts every rating made today, including ones of cards deleted or decks archived since. "Today"
  is the computer's current time zone, so changing it (or travelling) moves where the day begins.
- **Suggestions are a first step.** They come only from Ollama on this computer, from one note
  at a time, and from one part of up to 12,000 characters of a longer note, as front/back cards.
  A file is suggested from once it's imported as a note. There's no cloud option, no choice of how
  many cards, and no way to choose the parts yourself or to suggest from several at once. Synapse
  doesn't remember which parts you've used. Suggested cards aren't linked to the note, and
  suggestions aren't kept: once you leave without saving, they're gone.
  How good they are depends on the model: small ones often copy the passage as the answer. The
  check that a card comes from your note compares words, so it's loosest for languages written
  without spaces, such as Chinese or Japanese.
- **Frontend tests cover the risky interactions, not every screen.** The Rust core has unit tests
  (`cargo test`). On the React side, `npm test` runs the screens against a stand-in for the Rust
  core and checks reviewing (reveal before rating, the keyboard shortcuts, never rating twice),
  confirmations and where focus goes, restoring and out-of-date (stale) refusals, the locked
  sidebar during backup and restore, and the unsaved-changes prompt. Layout, looks, and the real
  Tauri window are still checked by hand.

## Architecture

- **`src/`** — the React UI. `api.ts` is the only file that calls Rust, through typed Tauri
  commands (and the window, to hold it open while something is unsaved). `Shell.tsx` is the
  sidebar and the bar above each screen. The screens are `Dashboard.tsx` (the Study Desk),
  `DeckDetail.tsx`, `ReviewSession.tsx`, `Notes.tsx`, `ArchivedDecks.tsx`, and `Backup.tsx`;
  `SuggestCards.tsx` is the note screen's *Suggest cards*, and `noteParts.ts` splits a note too
  long to suggest from whole into parts.
  `unsaved.tsx` tracks which form holds unsaved text and asks before it's discarded. Tests sit
  beside the code they cover (`*.test.tsx`), with shared helpers in `src/test/`.
- **`src-tauri/src/`** — the Rust core:
  - `lib.rs` — app setup and command registration; `main.rs` is a thin entry point.
  - `commands.rs` — the Tauri commands, the only way the frontend reaches the core.
  - `db.rs` — where the database lives, opening it, running migrations, and the one-time seed.
  - `scheduler.rs` — FSRS scheduling via the `fsrs` crate, with no database access.
  - `study.rs` — the deck dashboard, review sessions, recording reviews, and today's count.
  - `authoring.rs` — creating, renaming, archiving, and unarchiving decks; adding, editing,
    deleting, and restoring cards.
  - `notes.rs` — typed notes: writing, editing, soft-deleting, and restoring them.
  - `suggest.rs` — asking Ollama, on this computer, to suggest cards from a note, and checking
    its answer. It writes nothing; suggested cards are saved through `authoring.rs`.
  - `extract.rs` — reading the text of a file to import as a note, and the PDF helper (Synapse
    started with `--read-pdf-text`, see `main.rs`). `tests/pdf_helper.rs` runs the real binary as
    the helper.
  - `export.rs` / `import.rs` — writing a `.zip` backup, and checking and restoring one.
- **`src-tauri/migrations/`** — forward-only `sqlx` migrations (`0001`–`0010`), embedded in the
  binary at compile time.
