# Synapse

A local-first desktop flashcard app. Everything lives in one SQLite file on your own machine: no
account, no server, and nothing sent over the internet. Reviews are scheduled with [FSRS](https://github.com/open-spaced-repetition/fsrs-rs)
using its default parameters, or ones fitted to your own review history if you choose. If you run [Ollama](https://ollama.com) on the same computer, a model
there can suggest cards from your notes; Synapse talks to nothing else.

Tauri 2 + React + TypeScript on the front, Rust + `sqlx` on the back. Rust owns the database,
migrations, validation, scheduling, and every business rule; React only calls typed Tauri commands.

## What Synapse does today

- **Decks** — create your own, rename them, archive ones you're done with, and unarchive them
  again when you want them back. A built-in read-only
  **Sample deck** is there from first launch so there's something to study immediately.
- **Cards** — add front/back cards, edit them (which never disturbs their schedule), delete them, and
  restore ones you deleted. **Cloze cards** hide parts of a text, like
  `The {{c1::mitochondria}} makes {{c2::ATP}}`, with one card (and one schedule) per number.
- **Study** — start a review session for a deck, reveal the answer, and rate it **Again / Hard /
  Good / Easy**, with **Card 2 of 5** and a progress bar as you go. The whole session can be
  answered from the keyboard: **Space** reveals, **1–4** rate. FSRS computes the next due date, and
  every rating is appended to `review_logs`.
- **Today** — the Study Desk shows how many reviews you've recorded today and for how many
  different cards (*Today: 12 reviews · 9 cards*), counted since midnight on your computer's clock.
- **Statistics** — reviews per day over the last 30 days, true retention (the share of reviews of
  cards in the Review state not rated *Again*, with how many reviews it's based on), your cards by
  state (New, Learning, Review, Relearning), and how many cards fall due on each of the next 14
  days. All decks or one deck.
- **Personal scheduling** — under *Settings*, fit FSRS's parameters to your own review history
  once there are at least 400 reviews to learn from. Synapse shows how well the parameters in use
  and the fitted ones predict your reviews, and how the intervals would change; nothing changes
  until you apply them, and then only future ratings use them. *Reset to defaults* goes back.
- **Desired retention** — also under *Settings*, choose the chance of remembering a card FSRS
  aims for when it's due, from 70% to 97% (90% unless you change it), for all decks and, if you
  like, for single decks. Before you apply a value, Synapse estimates your reviews a day over the
  next 90 days at the value in use and at the chosen one, by simulating your own cards with
  FSRS's simulator. Only future ratings use the new value; due dates already set stay.
- **History is kept** — deleting a card, archiving a deck, and deleting a note are all *soft*: the
  rows stay in the database, a card keeps its FSRS state and review log, and a note keeps its text
  and its times. Every one of them can be brought back: a deleted card restored, an archived deck
  unarchived, a deleted note restored — each exactly as it was.
- **Export** — save a `.zip` backup of all your study data wherever you choose.
- **Import (restore)** — replace everything in Synapse with a backup made by Export, after Synapse
  has checked the whole file and you've confirmed. It never merges.
- **Notes** — type up your own plain-text study notes, or import them from a file, keep them in a
  local library, open and edit them, and delete ones you're done with. A deleted note leaves the library but is kept under
  *Deleted notes*, where *Read note* shows its text without changing it and *Restore note* brings
  it back unchanged. A note written in Markdown is
  shown formatted (headings, lists, emphasis, quotes, code, tables), or as its plain text if you
  prefer; it's stored and edited as plain text either way.
- **Cards from a spreadsheet** — add front/back cards to a deck from a CSV or TSV file, after
  checking every row, and save a deck's cards as a CSV file.
- **Picture cards (image occlusion)** — draw boxes over a picture (a diagram, a map, a labelled
  figure); each box is a card that asks what's under it.
- **Decks from Anki** — add the front/back, cloze and image occlusion notes of an Anki package
  (`.apkg` or `.colpkg`) as new decks of new cards, after a preview of what comes across and what's
  left out.
- **Import files as notes** — make a note from the text of a PDF, text (`.txt`), Markdown (`.md`),
  Word (`.docx`), or PowerPoint (`.pptx`) file. You check and edit the text before it's saved, and
  the original is kept under *Sources*.
- **Sources** — the documents you imported, kept as they were: read them page by page, open or save
  the original, and see the notes written from each. Suggested cards and quiz questions from such a
  note say which page they came from, and open it with the quote marked.
- **Pictures in notes and on cards** — up to 20 pictures under a note, and up to 4 on each side of a
  card, shown in review with the question or the answer.
- **Cards from notes** — with a note on screen, write a card yourself and add it to one of your
  decks. The card is an ordinary card; it isn't linked to the note.
- **Suggested cards (optional)** — have a model in Ollama, on your own computer, suggest up to ten
  cards from a note, each shown with the passage of the note it comes from. You check, edit, and
  remove them, then save the ones you want into a deck. Nothing is saved until you do, and the
  note never leaves your computer.
- **Quiz on a note (optional)** — have a model in Ollama, on your own computer, write up to eight
  multiple-choice questions about a note, and answer them one at a time (keys 1–4 work). After each
  answer you see the passage of the note it comes from; at the end, your score, and any question you
  missed can be added to a deck as a card. Nothing else is saved.
- **Help with hard cards (optional)** — after you answer Again on a card three times in a row,
  Synapse offers help with it once its answer is shown: a model in Ollama, on your computer, writes
  a mnemonic, a simpler explanation, an analogy, or the question asked another way, from the
  card's question and answer only. You can also ask for help on any card after revealing it. It's
  a draft; nothing is saved unless you choose *Add as a note*. On a picture card, a vision model
  in Ollama sees the picture with the part asked about framed (branch
  `claude/project-thread-uno5mp`, not yet released).

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

**Find your way around.** The sidebar has five places: the **Study Desk** (your decks, with the
first one that has cards due at the top), **Notes**, **Statistics**, **Archived decks**, and
**Backup & restore**.
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

**Pictures in notes and on cards.** A note can show up to 20 pictures under its text, and each side
of a front/back card up to 4 under that side's text. In the note or card form, *Add picture* opens
the system's file window (PNG, JPEG, GIF or WebP, up to 20 MB); a picture pasted while writing is
added too, where the system lets the app read it (see *Current limitations*). Each picture has
*Move earlier* and *Remove*, and the list is saved with the text, in that order, when you save;
changing the pictures counts as an unsaved change. A note shows its pictures under the text; the
deck's card list shows small ones under each side; in review the front's pictures come with the
question and the back's with the answer. A picture is always shown from Synapse's own copy, never
from a path or address in the text (Markdown's `![…](…)` is still only described). Removing a
picture from a note or card doesn't delete the file Synapse keeps, so a deleted note or card, or a
restored backup, still has its pictures. Cloze cards don't take pictures.

**Cloze cards.** In *Add card*, choose *Cloze* and write a text with the parts to hide marked as
`{{c1::answer}}`, or `{{c1::answer::hint}}` to show the hint in its place. Each distinct number is
its own card with its own schedule and review history; the same number used twice hides both parts
on one card. As you type, the form says how many cards the text makes (or what's wrong with it).
In review, the question is the text with that card's parts shown as `[...]` (or `[hint]`) and every
other part filled in; revealing shows the whole text with the answer highlighted. The text follows
a card's rules (required, up to 2,000 characters, at least one hidden part, each closed with `}}`),
and a card's kind can't be changed later. In the deck's lists a cloze card is shown with its
question and answer and tagged *Cloze 1*, *Cloze 2*, ….

*Edit card* on any cloze card opens its text, and the form says what saving will do. When you save:

- a number still in the text keeps its card, schedule, and history, with the new wording;
- a number no longer in the text has its card deleted, exactly as *Delete card* would (it goes under
  *Deleted cards*, history kept);
- a number new to the text gets a new card, due straight away — unless that number had a card that
  was deleted in this deck, which comes back instead, with its old schedule;
- a card you deleted yourself stays deleted while its number stays in the text.

A deleted cloze card can be restored only while its number is in the text, and only into the deck
where the text's other cards are; it comes back with the text as it is now. *Restore all* restores
the ones that can be. The cards of one text move together: ticking one selects all of them, and
*Move selected* refuses a move that would split them. The Sample deck has no cloze cards.

Cards of one text are kept apart when reviewing, as Anki buries siblings: once you rate one of
them, the others wait until the next day (local midnight), so the same text doesn't come up twice
in a row. The card you rated still comes back the same day if it falls due again. Their schedules
aren't changed, and the due counts on the Study Desk and the deck screen leave them out, so they
match what a review will show; if nothing else is due, the review ends.

**Picture cards (image occlusion).** *Add card* → *Picture* → *Choose a picture* (PNG, JPEG, GIF
or WebP, up to 20 MB), or paste one into the form. Drag over the picture to draw a box, drag a box
to move it, or its corner to resize it; with a box focused, the arrow keys move it, Shift with the
arrow keys resizes it, and Delete removes it. Each box is its own card with its own schedule; set
two boxes to the same card (*Card 1*, *Card 2*, … beside each box) to hide them together. A box
can have a label, what's under it, shown as the answer. An optional header is shown above the
picture on every card. *Hide all* covers every box on the question and marks the one asked about;
*Hide one* covers only that one. The picture is kept by Synapse (see *Files Synapse keeps*) and
travels in backups. In the deck's lists a picture card shows its header and label and is tagged
*Picture, box 1*, ….

In review the picture is shown with the card's box covered by a `?`; revealing uncovers it, with
its label. *Edit card* on any picture card opens the picture with all its boxes, and saving follows
the cloze rules above: a card whose boxes are all still there keeps its schedule, a card whose
boxes were all removed is deleted (history kept), and a new card is due straight away unless that
card had been deleted in this deck, which comes back instead. The picture itself can't be changed;
add a new picture card instead. Restoring, *Restore all*, moving together and keeping siblings
apart for the day work as for cloze cards. Help with a picture card is written by a vision model
that sees the picture (see *Get help with a card you keep forgetting*), and *Export CSV* leaves
them out (a spreadsheet can't carry the picture).

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
and the Sample deck's cards stay out of this entirely. With two or more deleted cards, *Restore all*
asks for confirmation, naming how many, then restores every one of them the same way, all together
or not at all. To bring back only some, tick *Select* on those deleted cards and choose *Restore
selected*: it asks first, naming how many, and restores them together or not at all (if any was
changed meanwhile, nothing happens and the screen reloads). A cloze or picture card that can't come
back yet has no *Select*.

**Search, select, and change several cards.** With two or more cards, the deck screen has *Search
cards*, which shows only the cards whose front or back contains what you type (ignoring capitals).
Tick *Select* on any cards, or *Select all* for every card the search shows, then *Delete selected*
or *Move selected*. Both ask first, naming how many (and saying if the search is hiding some of
them). Deleting works as deleting one card does. Moving takes the cards to another of your decks
with the same id, text, FSRS state, due date, reps, lapses, and review log, so each is due there
exactly when it would have been here. All the selected cards change together, or none do: if any
of them was changed meanwhile, nothing happens and the screen reloads. The Sample deck and
archived decks can't be moved to or from.

**Rename or archive a deck.** Also on the deck screen. Archiving asks for confirmation, then moves
the deck to *Archived decks*: it can't be reviewed, changed, or given new cards, and any unfinished
session in it is closed. Its cards and history are kept, and its name stays taken.

**Unarchive a deck.** In *Archived decks*, *Unarchive deck* asks for confirmation, then puts the
deck back on the Study Desk exactly as it was: same name, description,
cards, FSRS state, review log, and sessions. Its cards become due again only on the dates they
already had, no review session starts, and cards you deleted before archiving come back still
deleted — you can restore them from the deck once it's back.

**Export your data.** *Backup & restore* → *Export data*. Pick where to save the `.zip`.
It contains `manifest.json` (format, format version, schema version, app version, and export time),
`db/synapse.sqlite`, a complete snapshot of your database, and `assets/`, one entry for each file
Synapse keeps (see *Files Synapse keeps* below), named by its SHA-256. Every kept file is checked
against its hash as it's packed; a file that's missing or changed stops the export, with nothing
saved. This is backup format 2; backups made by v0.7.3 and older (format 1) have no `assets/`.

**Restore a backup.** *Backup & restore* → *Import data* → pick a `.zip` made by *Export data*. Synapse
checks the whole file first — only its own export format is accepted, and anything else is refused
with nothing changed. If the backup is sound, Synapse names it and asks whether to replace **all**
your current decks, cards, reviews, sessions, and notes with the backup's. *Keep current data*
changes nothing (export first if you want a copy). *Replace my data* swaps the database in one step,
keeping a rollback copy until the restored database has opened and passed its checks; the Study
Desk then reloads from the backup. A backup from an older version of Synapse is upgraded as it's
restored; one from a newer version is refused. A backup's kept files are checked too: each must be
listed in its database with the same size, hash, and kind of file, and every listed file must be
there. They join Synapse's own files before the database is swapped, and a restore that fails takes
back the ones it added. A v0.7.3 backup (format 1) still restores; v0.7.3 and older refuse a
format 2 backup as too new. While a backup is being checked, waits for your
answer, or is being restored — and while an export is being saved — the sidebar can't be used, so
nothing else can start in the meantime.

**Files Synapse keeps.** From migration 0016 Synapse can keep files, not just text: an imported
document kept as a *source* (its original file and the text of each page or slide), and pictures.
They live in an `assets` folder beside the database (with `SYNAPSE_DB_DIR`, inside that folder),
each named by the SHA-256 of its bytes, so a file is stored once however often it's used. Every file
is checked against its bytes, not its name: a picture must really be a PNG, JPEG, GIF or WebP (SVG
is refused), a PDF must start as one, Word and PowerPoint files must be zip packages, and text must
be none of those. A file is written before the database lists it and never changes afterwards; the
app shows pictures and PDFs only through its own `synapse:` address, by hash. Pictures in notes
and on cards (migration 0017) and picture cards (image occlusion, migration 0018) keep their
pictures here, and the Sources screen keeps imported documents.

**Text recognition.** From migration 0019 Synapse can read the text in a kept picture, and on the
scanned pages of a PDF source, with an OCR engine built into the app
([ocrs](https://github.com/robertknight/ocrs), pure Rust; its two models are bundled in
`src-tauri/models/` and compiled into the program, see the README there). Nothing is downloaded and
nothing leaves your computer. On a PDF page, the largest picture drawn on it is read: JPEG,
uncompressed or Flate samples (grey, RGB, CMYK) and black-and-white CCITT Group 4 scans, turned as
the page is; JPEG 2000 and JBIG2 pictures are reported as unreadable. Reading runs in a helper
process (Synapse started with `--recognise-text`), can be stopped, and saves each page as it is
read. What was read is kept in `recognitions`, beside the file, never in place of what extraction
read: a source's page still holds its extracted text (empty for a scan), and offers the
recognised text next to it, which is used to find quotes on that page. Rows are never changed;
reading again adds a newer one. The engine reads the Latin alphabet without accents (Italian "è"
comes out as "e") and reads handwriting poorly, so recognised text is always shown as such, for you
to check.

**Text in pictures.** Each picture in a note, and in the note and card forms, offers *Text in
picture*: Synapse reads it and shows what it found in a panel marked as recognised text, with *Copy*
and, in a form, *Add to note* (or *Add to front* / *Add to back*), which adds it to the end of the
text; nothing is saved until the form is. *Notes* → *Note from a photo* chooses a picture, reads
it, and opens a new note with that text and the picture attached, to check and give a title. On a
picture card, *Find labels* reads the picture and proposes a box over each line of text (split
where words are far apart, as labels side by side are) or each word, a little larger than the text and labelled with it, each its own card, to check, adjust or
remove before saving; text already under a box with the same label is skipped, and the 200-box and
200-character label limits hold. *Stop*, or leaving the screen, ends a reading.

**Reading with a local model.** For what the built-in engine can't read (accents, handwriting),
every place that reads text offers *Read with*: *Built in*, or a vision model you've installed in
Ollama on this computer (one whose capabilities include "vision", such as qwen2.5vl or
llama3.2-vision). Your choice is remembered on this computer. As with suggested cards, only
`http://127.0.0.1:11434` is asked, only local models are listed (checked again before a picture is
sent), and nothing is sent unless you ask. The picture, or the picture a PDF page draws, is
prepared by the same helper process, at most 1,600 pixels on its longer side. A model can invent
text, so its answer is cleaned of wrapping ("Here is the text:", code fences), refused if empty or
longer than a note, and, where the built-in engine has read the same picture or page, shown with how
many of its words the model's text shares. The text is kept as the newest reading, marked as read by
that model. A model is much slower than the built-in engine (minutes a page on a CPU). *Find
labels* always uses the built-in engine, since only it says where the text is.

**Sources.** *Sources* in the sidebar lists the documents Synapse keeps (kind, pages or slides,
size, and date), newest first, with deleted ones underneath and *Restore*. *Add source* keeps a file
without writing a note. Opening one shows its text a page (or slide) at a time, with previous, next
and a page to jump to, and the notes written from it. *Make a note* starts a note from its text;
*Open original* opens a copy in your computer's own program for that kind of file (the kept file is
never handed out, only a copy in a temporary folder); *Save a copy* saves it where you choose;
*Rename* changes its title; *Delete* asks first, keeps the file and pages, and can be undone with
*Restore*. A note written from a source says so under its title and links back to it, or says the
source was deleted. Suggested cards and quiz questions from such a note show "From page 7 of …"
(or the slide) under their quote: it opens that page over the screen with the quote marked, and
you can move to the pages around it. Cards still don't remember where they came from once saved:
the page link is there while you check the drafts or take the quiz.

**Import cards from a spreadsheet.** *Backup & restore* → *Cards in a spreadsheet* → choose a deck
→ *Choose a CSV file*. The front goes in the first column and the back in the second; other columns
are ignored. Commas, semicolons, and tabs all work, as do quoted cells with line breaks, a header
row such as *Front, Back*, and files saved as UTF-8 or by older versions of Excel. Synapse shows
every row before adding anything: rows that break a card's rules (a blank side, more than 2,000
characters) are named and left out, and so, unless you untick the box, are cards whose front is
already in the deck or on an earlier row. *Add* saves the rest all together, as new cards due
straight away. A row with a cloze text (`{{c1::…}}`) in the first column and nothing in the second
is a cloze text, checked as one typed in the deck screen and making one card per number. Up to
5,000 rows and 10 MB per file.

**Export cards to a spreadsheet.** In the same place, choose a deck → *Export as CSV*. You get a
standard CSV file with a *front* and a *back* column, which spreadsheets open and Synapse can import
again. A cloze text is written once, however many cards it made, with the text in *front* and *back*
left empty, so importing it makes it a cloze text again. Only the cards' text is saved, not their
schedules or reviews, so it isn't a backup.
A cell that starts with `=`, `+`, `-`, `@`, a tab or a line break gets a `'` in front, so a
spreadsheet shows it as text instead of running it as a formula; Synapse takes that `'` off again
when it imports the file, so its own exports come back exactly.

**Import decks from Anki.** *Backup & restore* → *Decks from Anki* → *Choose an Anki file*, and pick
an `.apkg` (exported from Anki, in its current format or "for older Anki versions") or a `.colpkg`
(a whole collection). Synapse reads the file without changing anything and shows what it found:
each Anki deck with the name of the new deck it becomes (a deck named like one you already have,
archived ones included, gets " (2)" added), how many cards it brings, and how many notes are left
out and why. Notes come across when their type has one front and one back, such as Anki's *Basic*,
*Basic (and reversed card)*, *Basic (optional reversed card)* and *Basic (type in the answer)*, or
your own types built the same way. A note becomes one card; tick the box to also add the reversed
card for notes that have one in Anki. Cloze notes (Anki's *Cloze*, or your own cloze types that ask
with one cloze field) become cloze texts, one card per deletion number, checked as a typed cloze
text is; their *Back Extra* (or any other field on the answer) is left out, and the preview says
how many had one. Image occlusion notes (Anki's *Image Occlusion*) become picture cards, one card
per box number, with their picture kept by Synapse, when the package was exported with media
included; rectangles come across as drawn, ellipses and polygons as the box around them, rotated
shapes unrotated, and text shapes not at all (the preview counts notes with any of these).
Formatting becomes plain text with its line breaks. Left out: image occlusion notes whose picture
isn't in the file or whose shapes can't be read, cloze notes Synapse can't read (nested deletions, an empty answer, a number over 999), other
note types, notes with an empty side or text, and sides or texts over 2,000 characters. *Add* creates every deck and card together, or nothing. Up to 50,000 cards and 1,000
decks at a time.

**Write notes.** *Notes* (in the sidebar, or *Open notes* on the Study Desk) → *New note*. A note
has a title (up to 200 characters) and a plain-text body (up to 100,000 characters); both are
required. Line breaks and indentation are kept, and only blank space at the very start and end is
trimmed. *Open note* shows it, *Edit note* changes it. The library lists the most recently edited
note first.

**Markdown in notes.** A note with Markdown in it (`# headings`, `**bold**`, `*italic*`, `~~struck~~`,
`- lists`, `1. lists`, `- [ ] tasks`, `> quotes`, `` `code` `` and fenced code, `---` rules, and
pipe tables) is shown formatted when you open it. *Formatted* and *Plain text* above the note switch
between that and the text exactly as stored, and Synapse remembers your choice. Line breaks inside a
paragraph are kept, and indented lines aren't treated as code, so notes typed as plain text look as
they did. HTML in a note is shown as text, never run. Links aren't opened: their address is shown
after their text. Pictures aren't loaded: their description is shown instead. Editing is always on
the plain text, and the stored note never changes by being shown formatted. A note with no Markdown
in it is shown as plain text, with no choice to make.

**Import a file as a note.** *Notes* → *Import file* → pick a PDF, text (`.txt`), Markdown (`.md`),
Word (`.docx`), or PowerPoint (`.pptx`) file. Synapse reads its text and opens it in the new-note form,
titled after the document (its own title if it has one, else the file name), with a line saying
what was read. Check the text, edit it as you like, and *Save note*; leaving or closing first asks
whether to discard it. Choosing the file keeps it as a *source* (see *Sources* below), whether or
not you save the note; saving the note records, with it, that it was written from that source. The
note itself is still an ordinary note you can change freely; the source keeps what the file said.

What's taken from each kind of file:

- **PDF** — the text of each page, in order, with lines that were only broken at the edge of the
  page joined up again. Pages with no text (usually scanned images) are named, so you can check
  nothing's missing. A PDF with no text at all, such as a scan, is still kept as a source, whose
  pages text recognition can read (see *Text recognition*); it can't become a note directly.
  Password-protected PDFs can't be read.
- **Text and Markdown** — the whole file, as written (UTF-8, UTF-16, or older Windows text). Markdown
  then shows formatted when you read the note.
- **Word** — the document's paragraphs, list items as `- ` lines, and tables a row per line. Headers,
  footers, footnotes, comments, and images are left out.
- **PowerPoint** — each slide's text in the order the slides are shown, under *Slide 1*, *Slide 2*,
  and so on, followed by its speaker notes. Images are left out.

Files up to 50 MB can be imported; a note holds up to 100,000 characters, so a longer text has to
be shortened before it's saved (Synapse says so). Older `.doc` and `.ppt` files need saving in the
newer format first. A PDF is read by a second, short-lived run of Synapse, so a damaged file can't
crash the app: it's just reported as a file that couldn't be read. Text over 12,000 characters can
still be saved as one note; *Suggest cards* then offers it in parts (Synapse says so).

**Search your notes.** *Search notes*, above the library, narrows it to the notes whose title or
text contains what you type, ignoring capitals (accented letters included), still most recently
edited first. It's plain matching: no fuzzy matches, and deleted notes aren't searched. *Clear* (or
Escape) brings back the whole library. Searching saves nothing, so leaving with text in the search
field doesn't ask about unsaved changes, and the search is gone when you come back.

**Search everything at once.** *Search*, in the sidebar, looks through every card in every active
deck (front and back) and every note (title and text) together. Each word you type matches the
start of a word, ignoring capitals and accents ("ete" finds "Été"), and every word must match.
Results come best first (a match in a card's front or a note's title counts more), at most 50,
with the matched words marked. *Show* narrows them to cards or notes, and for cards to one deck.
*Open in deck* shows the card in its deck, marked; *Open note* opens the note; *Back to search*
returns to the same search. Deleted cards and notes, and cards in archived decks, aren't found
until they're restored. Rust keeps a full-text index (SQLite FTS5) of cards and notes in step as
they're written; it's part of the database, so it travels in backups.

**Delete or restore a note.** Open a note → *Delete note* asks for confirmation, then takes the note
out of your library: it can't be edited or used to write a card. Nothing is lost — the note
appears under *Deleted notes* at the bottom of the library, with when it was deleted. *Read note*
opens it read-only (its text, formatted as usual, and its pictures) with *Restore note* as its one
action: no editing, cards, suggestions or quizzes until it's back. *Restore note*, there or in the
list, puts it back exactly as it was, in the same place in the list. Deleting a note never touches
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

**Take a quiz on a note.** With Ollama set up as above, open a note → *Quiz me* → choose a model →
*Start quiz*. The model writes up to eight questions, each with one right answer and three wrong
ones, under the same rules as suggestions: Ollama on this computer only, local models only, *Stop*
at any time, and a long note quizzed one part at a time. Each question must quote the passage of
your note its answer comes from, and Synapse leaves out (and tells you about) any whose quote isn't
in your note word for word, whose right answer isn't taken from its quote, or whose four answers
aren't all different. The answers are shown in a random order. Choose one with a click or with
1–4; the right one is then marked, with its passage under it, and Enter (or *Next question*) moves
on. At the end you see your score and every question you missed, with your answer and the right
one. Tick the ones you want, choose a deck, and *Add* them: each becomes an ordinary card, with the
question on the front and the right answer on the back, saved all together like suggested cards.
The quiz itself isn't kept anywhere. Leaving asks first only while you have missed questions that
haven't been added as cards, since those can't be got back; a quiz with nothing missed can simply be
written again.

**Get help with a card you keep forgetting.** A card's *Again streak* is how many of its latest
reviews, in the order they were recorded, were all rated Again; a Hard, Good or Easy ends it.
Every review of the card counts, in any session, including before it was edited or deleted and
restored. From three, the card shows *Get help with this card* once its answer is revealed; on any
other card the same button is there, quieter. Choose a kind of help and a model, then *Write help*.
The model reads only the card's question and answer, under the same rules as suggestions (Ollama
on this computer only, local models only, and *Stop* at any time). The help is a draft: *Add as a
note* saves it as an ordinary note, with the card under it. Rating the card never waits for it, and
rating closes the help, stopping it if it's still being written. Without Ollama, reviewing works
exactly as before.

On a picture card (branch `claude/project-thread-uno5mp`, not yet released), only Ollama's models
that can see pictures are offered, since the answer is in the picture: the model is shown the
picture with the part the card asks about framed in red, the card's header and that part's label.
If Ollama has no such model, Synapse says so rather than offering a model that can't see. Vision
models are slow without a graphics card: help may take a few minutes, and Synapse waits up to 10.

The Sample deck is read-only: you can study it, but not rename, archive, edit it, or add cards to it.

## Current limitations

- **Import replaces; it never merges.** Restoring a backup replaces all current data, and that can't
  be undone from the app. Only Synapse's own exports are accepted (Anki files and CSV are added as
  new cards elsewhere, never restored; raw `.sqlite` files aren't accepted), a backup whose database unpacks to more than 1 GiB is refused, and there's no
  preview of a backup's contents beyond its file name and export time. A restore that's interrupted
  (the process killed mid-swap) can leave a rollback copy in Synapse's data folder
  (`import-workspace/rollback.sqlite`, later renamed `rollback-kept-<time>.sqlite`); these are never
  deleted automatically.
- **Kept files only grow.** Files are never removed, even when what used them is deleted, and a
  restore leaves files only the replaced data used in the `assets` folder, unused (they take space,
  nothing else). Kept files are limited to 64 MiB each (20 MiB for a picture) and 2 GiB and 60,000
  files in all, so a backup stays a plain zip.
- **Pictures can't be dropped in, and pasting one depends on the system.** Tauri's window takes
  dropped files itself, so the forms offer *Add picture* and pasting only. Pasting a picture was
  tried only on Linux (WebKitGTK), where the page is handed no picture at all, so there only *Add
  picture* works; on Windows (WebView2) it is expected to work but hasn't been tried. Pictures
  aren't exported to CSV or brought in from Anki.
- **Restoring is per deck.** *Deleted cards* restores one card, the ones ticked with *Restore
  selected*, or with *Restore all* every deleted card of that deck at once; there's no *Select all*
  for deleted cards, no undo of a restore beyond deleting the cards again, and no way to move a card
  to a different deck on the way back.
- **Notes are plain text.** Markdown is shown formatted when you read a note, but there's no
  formatting toolbar or live preview while editing, no reference-style links, footnotes or maths, and
  links can't be opened from a note. No attachments, audio, web clips, tags, or folders. Search
  is exact text only (no fuzzy matching, no search inside deleted notes) and has no index, so each
  search reads every note.
  Importing a file makes a note from its text only: layout and images are left out of the note (the
  original is kept under *Sources*). Scanned pages are read by text recognition, which knows no
  accents and reads handwriting poorly. A card doesn't remember the note it was written or suggested from — so
  deleting a note leaves its cards exactly where they are. A deleted note can be read but not
  edited; restore it first.
- **No sync, accounts, cloud, or collaboration.** By design.
- **Personal scheduling is all or nothing.** One set of parameters is fitted to every review ever
  recorded (deleted cards and archived decks included) and used for every deck; there's no
  per-deck set and no automatic re-fitting: you optimize again yourself. Fitted parameters apply only to ratings made afterwards; cards already
  scheduled keep their due dates. Fitted parameters are only offered when they predict your own
  reviews better than the ones in use, measured on those same reviews.
- **Desired retention estimates are estimates.** They simulate your cards with the parameters in
  use and how often you choose each rating (FSRS's averages until there are 50 of each kind),
  assuming you review everything due and add no new cards; same-day repeats after *Again* aren't
  counted. The sample deck and archived decks always follow the value for all decks.
- **Three card types.** Basic front/back, cloze, and picture cards (image occlusion, boxes only:
  no freehand or polygon shapes). Front/back and cloze cards are text only: no images, audio, or
  tags. A card can't change type, and a picture card's picture can't be swapped. Anki's cloze and
  image occlusion notes are imported without their *Back Extra*.
- **Search matches words, not meanings.** *Search* finds words by their start (no fuzzy matching,
  no matching inside a word, no synonyms), never searches deleted cards or notes, and shows at most
  50 results. Words are split on anything that isn't a letter or digit, so text in scripts written
  without spaces (Chinese, Japanese) is only found from the start of a run. The deck screen's own
  *Search cards* and the library's *Search notes* are plain contains-text filters.
- **Bulk actions are delete, move and restore.** There are no tags or folders.
- **Statistics are counts, not scores.** The Study Desk counts today's reviews and the different
  cards they were for; the *Statistics* screen adds the last 30 days, true retention, cards by
  state, and a 14-day due forecast. There are no streaks, XP, or mastery percentages. Review
  history counts every rating made, including ones of cards deleted or decks archived since; cards
  by state and the forecast count only cards you can study now, and today's forecast also holds
  overdue cards and new cards not yet studied. Retention counts only reviews of cards in the Review
  state, so it's empty until cards have graduated. Days are the computer's current time zone, so
  changing it (or travelling) moves where each day begins. Only active decks can be chosen.
- **Spreadsheet files carry text only.** Importing reads only the first two columns as text and
  keeps no schedules, so it isn't a way to move a deck with its history. A cell another program
  wrote as `'=…` (a `'` and then a formula character) loses its first `'` on import, as Synapse's
  own exports need.
- **Anki imports bring text, not history.** Every imported card starts as new: Anki's schedule,
  review history, tags, flags, suspended state and note links aren't imported, and neither are
  pictures, sounds, LaTeX images or styling (text beside them is kept), except the picture of an
  image occlusion note. Note types with more than one field on either side, are left out and counted.
  Subdecks become separate decks named like `Parent::Child`. Importing the same file twice makes
  a second set of decks; nothing is merged or deduplicated.
- **Suggestions are a first step.** They come only from Ollama on this computer, from one note
  at a time, and from one part of up to 12,000 characters of a longer note, as front/back cards.
  A file is suggested from once it's imported as a note. There's no cloud option, no choice of how
  many cards, and no way to choose the parts yourself or to suggest from several at once. Synapse
  doesn't remember which parts you've used. Suggested cards aren't linked to the note, and
  suggestions aren't kept: once you leave without saving, they're gone.
  How good they are depends on the model: small ones often copy the passage as the answer. The
  check that a card comes from your note compares words, so it's loosest for languages written
  without spaces, such as Chinese or Japanese.
- **Quizzes are a first step too.** Only multiple choice, one note (or one part) at a time, and
  nothing about a quiz is kept: no scores, no history, and no link between a card added from a
  missed question and its note. The wrong answers are the model's own and aren't checked against
  the note, so a weak model can write one that is arguably also right; the passage shown after
  each answer is there to settle it.
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
  `DeckDetail.tsx`, `ReviewSession.tsx`, `Notes.tsx`, `Sources.tsx`, `Search.tsx`, `Statistics.tsx`,
  `ArchivedDecks.tsx`, `Backup.tsx`, and `Settings.tsx`;
  `SourcePages.tsx` shows a source's page with a quote marked, and the "From page 7 of …" links;
  `Occlusion.tsx` draws a picture card's boxes, for the deck screen's form and for review;
  `CardHelp.tsx` is the review screen's *Get help with this card*,
  `SuggestCards.tsx` is the note screen's *Suggest cards*, `Quiz.tsx` its *Quiz me*, and
  `noteParts.ts` splits a note too long to suggest from whole into parts.
  `pictures.tsx` shows kept pictures and is the forms' picture field (`KeptPicture`,
  `PictureGallery`, `PictureField`, `usePictureAdder`).
  `CardsCsv.tsx` and `AnkiImport.tsx` are *Backup & restore*'s *Cards in a spreadsheet* and
  *Decks from Anki*.
  `unsaved.tsx` tracks which form holds unsaved text and asks before it's discarded. Tests sit
  beside the code they cover (`*.test.tsx`), with shared helpers in `src/test/`.
- **`src-tauri/src/`** — the Rust core:
  - `lib.rs` — app setup and command registration; `main.rs` is a thin entry point.
  - `commands.rs` — the Tauri commands, the only way the frontend reaches the core.
  - `db.rs` — where the database lives, opening it, running migrations, and the one-time seed.
  - `scheduler.rs` — FSRS scheduling via the `fsrs` crate, with no database access.
  - `optimizer.rs` — fitting FSRS's parameters to the review history (off the UI thread, stoppable,
    and stopped after two minutes), and which parameters are in use. Each apply or reset is a new
    row in `fsrs_parameters`; nothing already scheduled is rewritten.
  - `retention.rs` — the desired retention for all decks and each deck, and estimating its
    reviews with FSRS's simulator. Each change is a new row in `desired_retention`.
  - `study.rs` — the deck dashboard, review sessions, recording reviews, and today's count.
  - `search.rs` — *Search*: every active card and note through the FTS5 index migration 0013
    keeps in step. It writes nothing.
  - `stats.rs` — the Statistics screen's numbers, read from the review log and the cards. It
    writes nothing.
  - `authoring.rs` — creating, renaming, archiving, and unarchiving decks; adding, editing,
    deleting, and restoring cards.
  - `cloze.rs` — cloze cards: reading a cloze text, writing its cards' fronts and backs, and the
    rules for adding, editing, and restoring them.
  - `occlusion.rs` — picture cards (image occlusion): checking boxes, writing each card's front
    and back, and the same add/edit/restore rules as cloze cards, by box number.
  - `notes.rs` — typed notes: writing, editing, searching, soft-deleting, and restoring them.
  - `suggest.rs` — asking Ollama, on this computer, to suggest cards from a note, and checking
    its answer. It writes nothing; suggested cards are saved through `authoring.rs`.
  - `quiz.rs` — asking Ollama, through `suggest.rs`'s client, for a multiple-choice quiz on a
    note, and checking each question against the note as suggested cards are. It writes nothing;
    missed questions are added as cards through `authoring.rs`.
  - `aid.rs` — a card's Again streak, and asking Ollama, through `suggest.rs`'s client, for help
    with a card. It writes nothing; help is kept as a note through `notes.rs`.
  - `extract.rs` — reading the text of a file to import as a note, and the PDF helper (Synapse
    started with `--read-pdf-text`, see `main.rs`). `tests/pdf_helper.rs` runs the real binary as
    the helper.
  - `cards_csv.rs` — reading CSV/TSV files of cards and writing a deck's cards as CSV.
  - `anki.rs` — reading an Anki package's collection (copied out to a temporary folder and opened
    read-only, never attached to Synapse's database) and adding its front/back, cloze and image
    occlusion notes (with their pictures) as new decks.
    `testdata/anki/` holds packages exported by Anki itself, and the script that made them.
  - `assets.rs` — the files Synapse keeps, named by hash in `assets/` beside the database: telling
    what a file is from its bytes, adding one, and serving pictures and PDFs on the `synapse:`
    scheme.
  - `pictures.rs` — the pictures a note, or a side of a card, shows: checking a list of them and
    saving it with the note or card (`note_pictures`, `card_pictures`).
  - `sources.rs` — imported documents kept as sources, with the text of each page; the notes
    written from each (saved with their link in one transaction); finding a quote on its pages;
    and copies of the original to open or save.
  - `ocr.rs` — the built-in text recognition engine: reading a picture, finding and decoding the
    scanned picture on a PDF page, and the helper process (Synapse started with
    `--recognise-text`) that does both. `tests/ocr_helper.rs` runs the real binary as the helper.
  - `recognition.rs` — what was read from each kept picture or PDF page (`recognitions`), reading
    a picture or a source's scanned pages through the helper, and stopping it.
  - `vision.rs` — reading a picture or a source's pages with a local vision model in Ollama
    (through `suggest.rs`'s client): cleaning and checking its answer, and its agreement with the
    built-in engine.
  - `export.rs` / `import.rs` — writing a `.zip` backup with every kept file, and checking and
    restoring one (kept files included).
- **`src-tauri/migrations/`** — forward-only `sqlx` migrations (`0001`–`0019`), embedded in the
  binary at compile time.
