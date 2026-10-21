# Synapse — complete project breakdown

Synapse is a **local-first** desktop study application: today, it is a working offline flashcard and note-taking tool with spaced-repetition scheduling, reversible content management, portable backups, and Windows installers. The longer-term goal is to turn a student’s own learning material into a private, controllable study environment with optional local and cloud AI.

Those are two different levels of maturity, and keeping them separate is essential:

- The current application is real, released, and usable without AI.
- The broader study-companion vision is the roadmap, not a list of features already implemented.

As of this update, the latest published release is v0.8.1, released from `main` by the Release workflow on October 21, 2026 (the `v0.8.1` tag names the commit): restoring selected deleted cards and reading a deleted note (sections 4.2 and 4.3). v0.8.0 (October 20, 2026) was the first major feature, "Sources and pictures" (sections 4.13–4.16): kept originals, pictures, image occlusion, and backup format 2. Since v0.5.0 (October 18, 2026), the 2026-10-19 and 2026-10-20 rounds shipped v0.5.1, v0.6.0, v0.7.0, v0.7.1, v0.7.2, v0.7.3 and v0.8.0, and the 2026-10-21 round v0.8.1 (section 13); their features are described in sections 4 and 5, some still marked with the branch they were written on. v0.5.0 added importing files as notes (section 4.7). v0.4.0 added the first AI feature: cards suggested from a note by a model in Ollama on the user's computer (section 4.6). v0.3.2 added unsaved-changes protection and the first frontend test suite (sections 4.5 and 12). The rest of this breakdown was first written against v0.3.1 and has been updated wherever those changes reach.

This breakdown combines that current repository documentation, your implementation reports, and the original product/specification documents. The older specification is useful for architecture and intent, but parts of its feature-status descriptions are outdated—for example, it predates the restore/unarchive work.

______________________________________________________________________

## 1. The project in one sentence

### What it is today

> Synapse is an offline desktop flashcard application that lets you write notes, create decks and cards, review due material using FSRS scheduling, recover deleted or archived content, and back up your complete study history.

### What it is intended to become

> Synapse is a personal study companion that turns your explicitly supplied learning material into flashcards, summaries, quizzes, explanations, and guided study sessions—using local processing where practical and optional cloud models when you choose them.

The original product promise is control: control over your study material, your model choices, your history, and whether anything leaves your computer.

### What it should not become

The product is not intended to be:

- A generic chatbot with a file-upload button.
- A cloud account wrapped around a flashcard generator.
- A dashboard filled with invented mastery statistics.
- A system that silently sends notes to a provider.
- A collection of impressive-looking but disconnected demos.
- A replacement for understanding the material.
- An attempt to train a new large language model.

The intended advantage is the combination of material ownership, active learning, transparent AI use, and persistent study history—not simply “it uses AI.”

______________________________________________________________________

## 2. The problem Synapse addresses

The project has two related problems to solve.

### 2.1 Studying requires more than storing information

A student can collect hundreds of pages of notes and still struggle to remember or explain them.

A useful study workflow needs to support:

1. Bringing material into a workspace.
2. Understanding and organizing that material.
3. Turning important ideas into questions.
4. Attempting answers without seeing them first.
5. Returning to material at appropriate intervals.
6. Detecting repeated difficulty.
7. Trying a different explanation when repetition alone is not working.

The current app already supports the manual question-and-review portion of that loop. The longer-term plan adds ingestion, generation, and alternative forms of explanation.

### 2.2 Students should retain ownership of the workspace

Your original requirements reject a situation where the usefulness of the app depends entirely on an online account or a single AI provider.

The user should be able to:

- Open their existing notes offline.
- Review cards offline.
- Keep recording review outcomes offline.
- Export their study environment.
- Choose whether to configure local AI.
- Explicitly choose whether to use cloud processing.
- Understand the limitations of either path.

The first version of Synapse solves this most directly by having no AI or network dependency at all. Later versions must preserve that foundation rather than making ordinary reviewing depend on a model being available.

______________________________________________________________________

## 3. Current verified project state

### 3.1 Repository and distribution

| Area | Current state |
| :-- | :-- |
| Repository | `Chip-jpg/synapse-flashcards` |
| Visibility | Public, by your deliberate choice |
| Development environments | Your local Windows setup, and Claude Code cloud sessions (Linux) |
| Latest published release | v0.8.1 (October 21, 2026) |
| Release commit | The commit the `v0.8.1` tag points at, on `main` |
| Windows installer | `Synapse_0.8.1_x64-setup.exe` |
| Windows MSI | `Synapse_0.8.1_x64_en-US.msi` |
| Installer signing | Release notes say the installers are unsigned |
| How releases are built | The manual **Release** GitHub Actions workflow on `windows-latest` (since v0.3.2; section 13) |
| Database migrations documented in current README | `0001`–`0018` |
| Frontend tests | Vitest + Testing Library: 271 tests at v0.8.1 (`npm test`; section 12.4) |
| AI generation | Cards suggested from a note by a local model in Ollama, since v0.4.0 (section 4.6) |
| File import | PDF, TXT, MD, DOCX, PPTX as notes, since v0.5.0 (section 4.7); originals kept as sources since v0.8.0 (sections 4.13, 4.16) |

The v0.5.0 release assets are uploaded:

- EXE: 3,245,341 bytes, SHA-256 `e41bfaece7a7d463c052f825df8154813348a480cc1a847117bf7cf9b860048f`.
- MSI: 4,505,600 bytes, SHA-256 `a1ad56d1172b8c6aa8b8dc44a9c3e9f1f46b603d7ec8160918a05d0a67b6a37d`.

The published files were downloaded and their hashes matched the ones the Windows build logged (and GitHub's own asset digests). That is verified release metadata, not a fresh installation test: none of v0.3.2, v0.4.0, or v0.5.0 has been installed on Windows yet.

### 3.2 Important distinction: repository, app, and personal data

Making the source repository public does not inherently upload your local study database.

There are three separate things:


| Thing | Purpose |
| :-- | :-- |
| Git repository | Application code and selected public project files |
| Installed application | Executable frontend/backend and packaged resources |
| Local user database | Your notes, decks, cards, schedules, and review history |

Your workflow has deliberately excluded personal databases, exports, private planning documents, agent workspaces, and design-reference archives from Git.

However, exclusion rules are safeguards—not substitutes for reviewing the staged files before each commit.

______________________________________________________________________

## 4. What the application can do today

## 4.1 Deck management

A deck is the primary organization unit for flashcards.

A normal deck can be:

- Created.
- Named.
- Given an optional description at creation.
- Opened.
- Renamed.
- Archived.
- Unarchived.

Cards belong to decks, and review sessions are scoped to a deck.

### Naming rules

The original authoring contract specifies:

- A required name.
- A maximum of 100 characters.
- Case-insensitive uniqueness.
- An optional description of up to 500 characters.

Archived deck names remain reserved. Archiving a deck does not free its name for a different deck.

### Archive behavior

Archiving means “remove this from my active study workspace,” not “destroy it.”

It preserves:

- Deck identity.
- Name and description.
- Cards.
- Card schedules.
- Review history.
- Session history.

It prevents ordinary active use of that deck while archived.

If the deck has an unfinished session, archiving closes that session as part of the operation. Unarchiving does not reopen it.

### Unarchive behavior

Unarchiving restores the deck to the active workspace.

It does not:

- Reset cards.
- Make all cards new.
- Create a session.
- Resume a closed session.
- Restore cards that were individually deleted.

Cards that were deleted before archiving remain deleted afterward. Once the deck is active again, those cards can be restored through the card-recovery flow.

### Sample deck

The built-in Sample deck lets someone try the review loop without creating their own material.

It is read-only through normal authoring flows:

- It can be studied.
- It cannot be renamed or archived.
- The user cannot add, edit, delete, or restore its cards through ordinary management controls.

The sample content is seeded once rather than repeatedly recreated on every launch. The seed-marker mechanism was part of the original persistence design.

______________________________________________________________________

## 4.2 Card authoring

The current card type is front/back text.

A card contains:

- A front: question, prompt, or cue.
- A back: answer or explanation.
- A deck assignment.
- Scheduling state.
- Creation and modification timestamps.
- Optional deletion state.
- Associated review history.

The original validation contract requires both sides and limits each side to 2,000 characters. Internal line breaks are preserved.

### Editing a card

Editing changes the wording, not the card’s learning history.

It should not reset:

- FSRS state.
- Due date.
- Stability.
- Difficulty.
- Repetition count.
- Lapse count.
- Review logs.

That means fixing a typo or improving an answer does not erase the fact that you have studied the card.

There is a future product question here: a completely rewritten card may represent a different fact and arguably deserve a new schedule. The current app does not try to infer that distinction. Editing preserves history; creating a genuinely new card is a separate action.

### Deleting a card

Deletion is soft deletion.

The card leaves:

- Active card lists.
- Active counts.
- Due-card selection.
- Ordinary reviewing.

Its row and review history remain in the database.

The user can find it under Deleted cards within its deck.

### Restoring a card

Restoration clears the deletion state without resetting learning data.

The restored card keeps its:

- ID.
- Front and back.
- Original deck.
- FSRS state.
- Stability and difficulty.
- Due date.
- Last-review value.
- Repetition and lapse counts.
- Review logs.

A restored card may be due immediately if its preserved due date is already in the past. That is different from resetting it to New.

### Restoration restrictions

The normal restore flow refuses:

- Missing cards.
- Cards that are already active.
- Cards in archived decks.
- Protected Sample deck cards.

The user must unarchive a deck before restoring one of its deleted cards.

### Restore all

With two or more deleted cards in a deck, *Restore all* (`restore_deleted_cards` in `authoring.rs`) restores every one of them in one transaction, by the same preservation rules and deck refusals. It sends the number the user confirmed; if the deck no longer has exactly that many deleted cards, it is refused as stale and nothing is written.

### Restore selected

Released in v0.8.1 (written on branch `claude/nightly-2026-10-21-restore-selected-58koby`). Under *Deleted cards*, each card that can be restored has a *Select* box; *Restore selected* (`restore_flashcards` in `authoring.rs`) restores the ticked ones in one transaction, by `restore_flashcard`'s rules (Sample deck, archived deck, `cloze::restore_block` and `occlusion::restore_block`, checked card by card; cloze and picture fronts and backs rewritten). It sends the ids the user confirmed; if any is no longer a deleted card of that deck, it is refused as stale (`SelectionChanged`) and nothing is written. No migration.

### Several cards at once

Added after v0.6.0. On the deck screen, *Search cards* filters the deck's active cards by exact text in front or back, ignoring case (in React: the cards are already loaded and nothing is written). Cards are ticked one by one or with *Select all* (the cards the search shows), then *Delete selected* (`delete_flashcards`) or *Move selected* (`move_flashcards`, both in `authoring.rs`). Each sends the ids the user confirmed and works in one transaction; if any of them is no longer an active card of that deck, it is refused as stale and nothing is written. A move changes only `deck_id` and `updated_at`, so FSRS state, due date, counts and review logs carry over. The Sample deck and archived decks are refused on either side. No migration: 0006 already refuses moves into or out of an archived deck, and 0010 a move of a deleted card.

### Timestamp caveat

Your implementation report identified one historical detail worth retaining: deleting a card already changes its `updated_at` timestamp. Restoring it cannot recover an earlier edit timestamp that was not stored separately.

Therefore, “restored exactly as it was” is best understood as preserving the state held in the deleted row—not reconstructing a timestamp value from before deletion.

______________________________________________________________________

## 4.3 Notes

Notes are a global plain-text library.

They are not currently folders of attached documents, nor are they inherently tied to a deck.

A note can be:

- Created.
- Opened.
- Edited.
- Soft-deleted.
- Restored.


### Note content

A note has:

- A title.
- A plain-text body.
- Creation time.
- Last-edit time.
- Optional deletion time.

Current documentation specifies:

- Title: required, up to 200 characters.
- Body: required, up to 100,000 characters.
- Outer whitespace trimmed.
- Internal indentation and line breaks preserved.

The library is ordered with recently edited notes first. These limits and preservation rules originate in the typed-notes contract.

### Deleted notes

A deleted note is removed from ordinary navigation and cannot be opened or edited through normal note commands.

The recovery list shows its identifying information and dates. Since v0.8.1 (written on branch `claude/nightly-2026-10-21-deleted-note-preview-tcxs3m`), *Read note* opens it read-only (`get_deleted_note`, `notes::deleted_note`: title, body, dates, pictures; refused as stale once restored), formatted as an active note is, with *Restore note* as its only action. Editing, card from note, suggestions and quizzes still need it restored, and Rust still refuses edits to a deleted note. No migration.

Deletion and restoration preserve the note’s content and original edit timestamps.

### Notes and cards are independent

“Create card from note” currently means:

1. Keep a note visible.
2. Choose an active normal deck.
3. Write a front and back manually.
4. Save through the normal card-creation path.

It does not mean:

- Generate cards with AI.
- Automatically fill in the answer.
- Create a durable source link.
- Keep a count of cards linked to a note.
- Update cards when the note changes.

This is an important boundary for both UI and future AI work. The original manual note-to-card slice explicitly chose independent, self-contained cards.

______________________________________________________________________

## 4.4 Review sessions

Reviewing is the central working loop:

```text
Choose a deck with due cards
          ↓
Start or resume its session
          ↓
Read one card’s front
          ↓
Try to recall the answer
          ↓
Reveal the back
          ↓
Choose Again / Hard / Good / Easy
          ↓
Persist the outcome and next schedule
          ↓
Continue until nothing in that deck is due
```

The current app is not testing the answer automatically. The user rates their own recall.

### Ratings

| Rating | Keyboard key |
| :-- | --: |
| Again | 1 |
| Hard | 2 |
| Good | 3 |
| Easy | 4 |

Space or Enter reveals the answer, subject to normal focus handling.

The keyboard implementation was designed to avoid accidental reviews:

- Rating keys do nothing before reveal.
- Repeated key events are ignored.
- Modifier-key combinations are ignored.
- Inputs must not cause a double submission.
- Rating keys are blocked while a review is saving.
- Buttons remain usable.


### Progress

The review experience includes:

- A display such as “Card 2 of 5.”
- A progress bar.
- A completion summary.

The total is dynamic rather than frozen when the session starts.

If another card becomes due while you are studying, the remaining workload can grow. The display represents current session progress against current due work, not a fixed assignment created at session start.

### Session persistence

Leaving the screen or closing the app does not inherently discard a valid unfinished session.

Returning to review that deck can resume it.

A session ends when no cards in that deck are due. Archiving the deck also closes its unfinished session.

### Reviews versus unique cards

One conceptual point matters for future statistics: a count of review actions is not necessarily a count of distinct cards.

If a card is reviewed more than once, those are multiple learning events. A future dashboard should distinguish “reviews completed” from “unique cards studied” rather than use the labels interchangeably.

______________________________________________________________________

## 4.5 Unsaved-changes protection

Added in v0.3.2.

A form is *unsaved* when what it holds differs from what it opened with. This covers all five forms: create deck, rename deck, add/edit card, new/edit note, and card from note (choosing a deck counts).

While any form is unsaved:

- Choosing a place in the sidebar asks "Leave without saving?" first.
- Closing the window asks "Close Synapse without saving?" first.

The answers:

- *Discard changes* carries on: to the chosen place, or closing the app.
- *Keep editing*, or Escape, returns to the form with its text and focus intact.

Boundaries:

- An untouched form, or one changed back to how it opened, doesn't ask.
- *Cancel* still discards without asking; it is an explicit choice.
- Nothing is saved as a draft. Discarding loses the text, and nothing survives a restart. This is a navigation guard, not autosave.
- The window close is held only while something is unsaved. Otherwise the window closes exactly as before.

How it works:

- Forms report their state through `useUnsavedChanges` in `src/unsaved.tsx`, and `App.tsx` asks before sidebar navigation.
- `api.ts` holds the window close through Tauri's close-requested event (`holdWindowClose`) and closes the window after Discard (`closeWindow`).
- Closing after Discard needs the `core:window:allow-destroy` capability in `src-tauri/capabilities/default.json`.
- The native close path was checked on Linux (WebKitGTK) before release, but not yet on Windows.

## 4.6 Suggested cards (local AI)

Added in v0.4.0. The first AI feature, deliberately narrow.

The flow: open a note → *Suggest cards* → choose a model → wait (with *Stop*) → check the drafts → choose a deck → save.

- **Only Ollama on this computer.** Synapse asks `http://127.0.0.1:11434` and nothing else, and only when the user presses *Suggest cards*. The HTTP client (`reqwest`, no TLS backend) ignores proxies and follows no redirects. Ollama's cloud models (marked `remote_host`/`remote_model`, or named `…:cloud`/`…-cloud`) are never listed, and the chosen model is checked against the local list again before the note is sent. The cloud filter follows Ollama's documented fields; it hasn't been tested against a real cloud entry.
- **Drafts, not writes.** Nothing is stored until the user saves. Drafts aren't kept anywhere, so the schema and backup format are unchanged (no migration). Suggested cards count as unsaved changes.
- **Kept to the note.** A model can't be made to forget what it knows, so what's checked is where each card says it came from. The model must quote, for each card, the passage its answer comes from. `suggest.rs` keeps a card only if the quote is in the note word for word (`quoted_from`), its answer is drawn from the quote (`drawn_from`: at least half its significant words), and the answer doesn't just repeat the question. Each kept card shows its quote, and lists answer words the note never uses (`unfamiliar`). These checks catch most outside facts, not all; the user still reads every card.
- **Saving is all or nothing.** `create_flashcards` (in `authoring.rs`) checks every card as a typed card is checked and saves them in one transaction. A refused card comes back with its `index` and `field`, and the frontend keeps that error on the right draft by its key.
- **Limits.** Up to 10 cards; notes up to 12,000 characters (`SUGGESTION_NOTE_MAX_CHARS`). Longer notes are refused, not cut, and so is a prompt that fills the 16,384-token context. Temperature is 0.
- **Stopping.** *Stop*, or leaving the screen, drops the request, and Ollama cancels the generation. Checked in Ollama's log.
- **Checked against real models.** Ollama 0.40.6 with qwen2.5:0.5b, llama3.2:1b and llama3.2:3b. Every quote was verbatim, and the stricter instructions alone stopped the 1B model adding facts the note doesn't state. Small models still write weak cards; 3B and up are noticeably better. A skipped test (`real_ollama_lists_the_model_and_suggests_cards`) asks a real Ollama.

## 4.7 Importing files as notes

Added in v0.5.0. *Notes* → *Import file* reads a PDF, TXT, MD, DOCX or PPTX and opens its text in the new-note form to check, edit and save.

- **The product decision (v0.5.0): text only, originals not kept.** Since v0.8.0 the original is kept as a source (sections 4.13 and 4.16); the rest of this section still describes how the text is read. An import becomes an ordinary note. Synapse doesn't copy the file, link to it, or remember where it was, so the schema and backup format don't change. This is the "smaller import-as-text feature" of section 10.6. Keeping originals, which the original vision prefers, remains a separate, larger feature.
- **What's read** (`extract.rs`):
  - PDF: page text in order, with lines broken at the page edge rejoined. Pages without text are reported, and a PDF with no text at all is refused (no OCR).
  - TXT/MD: UTF-8, UTF-16 with a byte order mark, or else Windows-1252. Markdown stays as written, and its title comes from front matter or the first heading.
  - DOCX: body paragraphs, list items as `- `, table rows on one line separated by tabs.
  - PPTX: slides in shown order, with speaker notes.
  - The title is the document's own title, or else the file name.
- **PDFs are read in a helper process.** `pdf-extract` panics on some files, and release builds use `panic = "abort"`. So Synapse starts itself with `--read-pdf-text` (handled in `main.rs` before Tauri), pipes the PDF in and reads JSON back. A crash, or a run past 60 seconds, is just "couldn't read that PDF". `tests/pdf_helper.rs` runs the real binary as the helper.
- **Limits.** Files up to 50 MB. Text over a note's 100,000 characters is shown with a warning and must be shortened before saving. `.doc`/`.ppt` aren't supported. Suggestions are limited to 12,000 characters, so long imports need splitting before *Suggest cards*.
- Checked in the real app on Linux, through the GTK file window, with files made by LibreOffice, pandoc and python-pptx. Not yet checked on Windows.

## 4.7b Decks from Anki, and safe CSV export

Added on branch `claude/nightly-2026-10-19-anki-import-t3vnsu` (not yet released). *Backup & restore* → *Decks from Anki* reads an `.apkg` or `.colpkg` and adds its front/back notes as new decks of new cards (`anki.rs`, `AnkiImport.tsx`). No migration; backups unchanged.

- **Reading.** Only the collection entry is unpacked, into a temporary folder removed afterwards: `collection.anki21b` (zstd, via the pure-Rust `ruzstd`) first, then `collection.anki21`, then `collection.anki2` (newer packages add a "please update Anki" stub as `.anki2`). Limits: 2 GB file, 512 MB unpacked collection. A WAL-marked copy is switched to a rollback journal, then opened read-only and immutable on its own connection, never attached to Synapse's database. Schema 11 (JSON in `col`) and 18 (tables, protobuf settings) are both read.
- **Which notes.** Decided by templates, not names: the first template shows one field on the question and adds one on the answer; other templates repeat or reverse it. Reversible only when Anki made the reversed card. Image occlusion notes become picture cards since branch `claude/sources-image-occlusion-8pmk5r` (section 4.14); other types, empty sides and sides over 2,000 characters are left out and counted.
- **Cloze notes** (branch `claude/nightly-2026-10-20-anki-cloze-01y1nm`, not yet released). A note type of Anki's cloze kind whose one template asks with exactly one `{{cloze:Field}}` and nothing else becomes a cloze text (section 4.11): HTML to text, then `cloze::validate_cloze`, one card per number, saved by `cloze::insert_cloze` in the same transaction. Anki's syntax is Synapse's. Other answer fields (Back Extra) are dropped; notes where they had text are counted (`extraRemoved`). Texts Synapse refuses (nested deletions, empty answer or hint, number over 999) are counted as `badCloze`; blank and too long as `empty`/`tooLong`. A normal note type using `{{cloze:}}` is "other type" (Anki doesn't number it). Fixtures add two cloze notes (one with a hint, a repeated number, HTML and Back Extra), a nested one, and an image occlusion note. HTML becomes plain text (`html_to_text`); pictures and sounds are dropped and counted.
- **Saving.** Preview first; Rust holds the package under a token (`AnkiImports`). Saving creates one deck per Anki deck with cards (`Parent::Child` names, made unique against every existing deck including archived, within 100 characters, description "Imported from Anki."), forward cards then reversed ones, in one `BEGIN IMMEDIATE` transaction. All cards start New; Anki scheduling, history, tags and media are not imported. Up to 50,000 cards and 1,000 decks.
- **Tested against real Anki output.** `src-tauri/testdata/anki/` holds packages exported by Anki 25.09 (PyPI `anki`) in all three formats, and `make_fixtures.py` that made them.
- **CSV formulas (v0.7.0's known issue, fixed).** `cards_csv.rs` exports a cell starting with `=`, `+`, `-`, `@`, tab or CR with a `'` in front (`protect_formula`), and also one that starts with `'` then such a character. Import strips exactly one such `'` (`unprotect_formula`), so Synapse's own exports round-trip; any other cell is read as is.
- **CSS fix found on the way.** `App.css` in v0.7.0 lost the `}` closing `.chart-axis`, so every rule after it (the CSV preview table, the check boxes, card help's choices) was nested under `.chart-axis` and never applied. Restored.

## 4.8 Help with hard cards (memory aids)

Added on branch `claude/nightly-2026-10-19-memory-aids-i9x6j9` (not yet released). The first part of section 14.4.

- **When it's offered.** A card's *Again streak* (`aid::again_streak`) is the number of its latest `review_logs` rows, ordered by `id`, that all have rating 1; any other rating ends it. It counts across sessions and through edits, deletion and restoration, since none of those touch the logs. At `HELP_AFTER_AGAINS` (3) or more, `get_session_card` sends `offerHelp: true` with the card (`againStreak` too), and the review screen offers help once the answer is revealed. The same help is a quiet button on every other card after reveal.
- **What it is.** A mnemonic, a simpler explanation, an analogy, or the question reframed, written by a local Ollama model from the card's front and back only (`aid.rs`, through `suggest.rs`'s `Ollama::ask`: same address, local-model check and Stop). Help over 1,500 characters, blank, or only repeating a side is refused; for an explanation or a question, words the card never uses are listed.
- **Drafts only.** Nothing is stored unless the user picks *Add as a note*, which uses `create_note`. No table, no migration, backups unchanged.
- **Never in the way.** Rating doesn't wait for help; rating replaces the panel and stops a request still running. Without Ollama, review is unchanged.

## 4.9 Quiz on a note

Added on branch `claude/nightly-2026-10-19-note-quiz` (not yet released). Note → *Quiz me* → choose a model → *Start quiz*.

- **Same rules as suggestions.** `quiz.rs` asks through `suggest.rs`'s `Ollama::ask`: 127.0.0.1 only, local models only (checked again before sending), user-initiated, *Stop* (`stop_quiz`, its own `Quizzing` state), 12,000-character limit with `noteParts` splitting. No migration, no table; backups unchanged.
- **Checked like suggested cards.** Up to 8 questions (`MAX_QUESTIONS`), each with an answer, exactly 3 distractors and a quote. Kept only if the quote is in the note word for word (`quoted_from`) and the answer is drawn from it (`drawn_from`), both now `pub(crate)` in `suggest.rs`; the four options must all differ (`comparable`), so the answer is never a distractor. Distractors aren't checked against the note.
- **Taking it.** One question at a time; options shuffled in React (`shuffled`); 1–4 answer (modifier and held keys ignored), then the right option is marked with the quote, and Enter/Space or *Next question* moves on. Summary: score and each missed question with both answers.
- **Missed questions → cards.** Ticked ones are added to a chosen deck with `create_flashcards` (front = question, back = right answer). Nothing else is stored.
- **Unsaved changes.** Leaving counts as unsaved only while some question has been missed and not added as a card: which questions were missed can't be got back, but the quiz can be written again. Adding the cards returns to the note.

## 4.10 Searching everything

Added on branch `claude/nightly-2026-10-19-search-everywhere-tombun` (not yet released). *Search* in the sidebar searches every active card (front and back, decks not archived) and every active note (title and body) at once.

- **Index.** Migration `0013` adds `search_index`, an FTS5 table (`unicode61 remove_diacritics 2`) holding each card's front/back and each note's title/body, backfilled by the migration and kept in step by triggers on insert, text update and (never used) delete. Rowid `2 * id` is a card, `2 * id + 1` a note. Deleted, archived and restored items stay indexed; `search.rs` filters them from the tables, so lifecycle changes never touch the index.
- **Query.** User text is split into letters-and-digits words (at most 10), each quoted as a prefix (`"word"*`), all required, so no FTS5 syntax can be typed. bm25 ranking, front/title weighted 2:1. At most 50 results plus `more`. Snippets come as `{ text, hit }` pieces, rendered with `<mark>`, never as HTML.
- **Filters.** Cards and notes, cards, or notes; for cards, one active deck (archived: stale).
- **Navigation.** A card opens its deck with that card marked and its Edit button focused; a note opens directly; *Back to search* returns to the same search and result.
- **Backups.** The FTS tables travel in exports (`VACUUM INTO`) and pass import's schema check; a v0.7.0 backup (schema 11) is indexed on restore. An older Synapse refuses a backup made after 0013 as newer than it knows.
- The per-deck *Search cards* and *Search notes* filters are unchanged.

## 4.11 Cloze cards

Added on branch `claude/nightly-2026-10-19-cloze-cards-ddsgr2` (not yet released). A second card type, the first item of section 14.2's "Cloze questions".

- **Writing one.** *Add card* → *Cloze*. Deletions are `{{c1::answer}}` or `{{c1::answer::hint}}`, numbers 1–999. Each distinct number is one card; a number used twice hides both parts on one card. The text is checked as a card is (required, 2,000 characters after trimming), must hold a deletion, each closed with `}}` before the next starts, with a non-blank answer (and hint). Anything else that looks like braces is plain text. The form shows a live count from Rust (`preview_cloze`), never parsed in React.
- **Schema (migration 0012).** `cloze_texts (id, text, created_at, updated_at)` holds each text once; its rows are never removed. Each card is an ordinary `flashcards` row with `kind = 'cloze'`, `cloze_text_id`, and `cloze_number` (basic cards: `'basic'`, NULL, NULL). Triggers: kind, text id and number are complete on insert and never change; a text is never deleted or blank; at most one *active* card per (text, number) (partial unique index). 0010's `flashcards_deleted_are_read_only` is recreated with the three new columns.
- **Fronts and backs are stored.** Rust writes a cloze card's `front` (its own deletions as `[...]`/`[hint]`, others filled in) and `back` (its answers, joined by "; ") whenever the text is saved. So lists, search, counts, statistics, help with a card and exports keep working unchanged. Review gets the text as pieces (`SessionCard.cloze`) to show the gap and the highlighted answer.
- **Editing** (`update_cloze`): a number still in the text keeps its card and schedule (front/back rewritten); a number that left the text soft-deletes its card; a number new to the text restores its deleted card from the same deck, if it has one, or adds a new card. A card the user deleted alone stays deleted while its number stays. A cloze card can't be edited as front/back, or the other way round (`WrongKind`).
- **Restoring.** A deleted cloze card is restorable only if its number is in the text now and the text's active cards are in its deck (`cloze::restore_block`: `numberGone`, `elsewhere`). It comes back with its front/back rewritten from the current text (a second statement after clearing `deleted_at`, as 0010 requires). *Restore all* counts and restores only restorable cards.
- **Moving.** The cards of one text stay in one deck: `move_flashcards` refuses a selection with only some of a text's active cards (`ClozeSplit`), and ticking one selects its siblings.
- **CSV.** Export writes each text once, as `front` with an empty `back`. Import treats a row with an empty back and a deletion in the front as a cloze text (checked the same way, one card per number); duplicates are judged against the deck's texts.
- **Backups.** A v0.7.0 backup (schema 11) restores with every card `basic`. A backup made after 0012 is refused by v0.7.0 and older as from a newer version ("newer schema"), as any newer backup is.
- **Siblings kept apart** (branch `claude/nightly-2026-10-20-cloze-siblings-k8it5x`, not yet released). Once a cloze card is rated, the other cards of its text aren't due until the next local day (`study::buried_cards!`, from today's `review_logs` since `bury_since`, i.e. `day_bounds` on the computer's zone; UTC in tests). The rated card itself isn't held back, so an Again comes back when due; two siblings both rated today don't hold each other back. Nothing is stored and no schedule changes; no migration. The session, its progress, `record_review` (a held-back card is stale), the Study Desk and deck-screen due counts all use the same rule. The statistics forecast and the retention estimate are about schedules and still count them.
- **Not done:** no conversion between kinds; no cloze suggestions from Ollama. Image occlusion is its own kind (section 4.14). Not yet checked on Windows. Anki cloze notes import as cloze texts since the 2026-10-20 branch (section 4.7b).

## 4.12 Markdown in notes

Added on branch `claude/nightly-2026-10-20-markdown-notes` (not yet released). Frontend only: no command, no migration, backups unchanged.

- **Reading.** Opening a note parses its body with `src/markdown.ts` (`parseMarkdown`, no dependency) and draws it with `src/NoteMarkdown.tsx`, React elements only, so no part of a note is read as HTML. Headings (ATX and underlined; level n becomes `h(n+2)` below the title's h2), paragraphs, emphasis, strong, `~~strikethrough~~`, code spans, fenced code, quotes, bullet/numbered/task lists, rules, pipe tables, links, autolinks and pictures. Leading front matter is shown as a code block.
- **Differences from CommonMark, on purpose.** Line breaks inside a paragraph are kept and indented lines are not code, so plain-text notes look as typed. No reference links, footnotes, HTML or entity decoding.
- **Links and pictures** are described, never opened or loaded: a link shows its text then its address; a picture shows "Picture: alt (address)".
- **Choice.** *Formatted* / *Plain text* above the body (`aria-pressed`), remembered in web storage (`synapse.noteFormat`, a per-computer convenience). Offered only when formatting changes something (`hasFormatting`); otherwise the body is shown as plain text as before.
- **Unchanged.** Editing is on the plain text; the stored body, search, card-from-note context, suggestions and quizzes (which quote the plain text) are untouched.
- **Bounded.** Parsing is linear-ish on the 100,000-character limit: brackets and backtick runs are matched in one pass, link addresses are looked for at most 1,024 characters on, and nesting stops at 32 levels.

## 4.13 Kept files and sources (foundation)

Released in v0.8.0 (written on branch `claude/sources-foundation-yugkjq`): phase 1 of "Sources and pictures": the storage, commands and backups that sections 4.14–4.16 build on. The contract the phase 2 screens were built to is in `/mnt/project-files/planning/sources-foundation-contract.md` (project files, not the repository).

- **The store** (`assets.rs`). Files live in `<database folder>/assets/<sha256>`, lowercase hex of their bytes, so equal files are stored once. Migration 0016's `assets` table lists each (`sha256`, `media_type`, `size`); rows are never changed or removed. The type is checked against the bytes (`matches_type`): pictures and PDFs by their signature (a picture's type is read from it, `sniff`; SVG refused), Word/PowerPoint must be zip packages, text must be neither. A file is written whole (`.partial-…`, fsync, rename) before its row; a missing file is written again when the same bytes are added. Limits: 64 MiB a file (20 MiB a picture), 2 GiB and 60,000 files in all, so a backup stays under zip's 4 GiB / 65,535 entries without zip64.
- **Showing a file.** `synapse://localhost/<sha256>` (Windows: `http://synapse.localhost/<sha256>`, through `keptFileUrl`) serves pictures and PDFs only, after checking the row and the file's size; anything else is a 404. Nothing is read by path.
- **Sources** (`sources.rs`). An imported document kept: its file (`asset_id`), `kind` (as `extract::FileKind`), title, file name, and `source_pages` (the text of each PDF page or slide; one page for other kinds), written together. Only the title changes (rename); delete and restore are soft, as notes. `source_notes` records, once and for good, which source a note was saved from.
- **Backups** (format 2, section 10.1). Export packs every listed file, checking size and hash as it reads. Restore checks the entries against the backup's own `assets` rows (name, size, hash, type; nothing missing or extra), stages them, adds them to the store before swapping the database, checks every listed file is present after opening, and takes back the files it added if anything fails.
- **Not done.** Files are never removed, and a restore leaves files only the replaced data used (no clean-up). Not checked on Windows; the `synapse:` scheme was checked in the real app on Linux (a picture stored, shown 1×1, served as `image/png`, missing hash a 404).

## 4.14 Pictures in notes and on cards

Released in v0.8.0 (written on branch `claude/sources-pictures-04zgws`), on top of 4.13: phase 2 of "Sources and pictures".

- **What.** A note shows up to 20 kept pictures under its text (`NOTE_PICTURES_MAX`), each side of a front/back card up to 4 (`CARD_SIDE_PICTURES_MAX`), in order. Added with *Add picture* (`choose_picture`) or pasted (`store_picture`), ordered and removed in the form, saved with the text in one transaction (`create_note`/`update_note`/`create_flashcard`/`update_flashcard` take an optional `pictures`; left out keeps them). Shown in the note, small in the deck's card list, and in review (front with the question, back with the answer; `SessionCard.pictures`).
- **Schema (migration 0017).** `note_pictures (note_id, position, asset_id)` and `card_pictures (card_id, side, position, asset_id)`, by asset id only. Saving replaces a list (delete + insert of link rows); the files and `assets` rows stay. Triggers: only image assets; link rows frozen while the note or card is deleted, or the card's deck archived; only active basic cards take pictures (cloze and later kinds don't). The Sample deck is refused in Rust. No column added to an existing table; backups unchanged (every `assets` row travels).
- **Frontend.** `src/pictures.tsx`: `KeptPicture` (falls back to "This picture couldn't be shown"), `PictureGallery`, `PictureField`, `usePictureAdder`. Picture changes count as unsaved.
- **Checked** in the real app on Linux (adding through the GTK file window, note view, card list, review). **Not working there:** pasting a picture (WebKitGTK hands the page no picture data); untested on Windows. Dropping files isn't offered (Tauri takes file drops). No pictures in CSV or from Anki yet.

## 4.15 Image occlusion cards

Released in v0.8.0 (written on branch `claude/sources-image-occlusion-8pmk5r`): phase 2 of "Sources and pictures", for v0.8.0. A third card type, modelled on cloze (section 4.11).

- **Writing one.** *Add card* → *Picture* → choose or paste a picture (kept through `assets.rs`: `choose_picture`/`store_picture`). Boxes are drawn, moved and resized with the mouse, or with arrow keys (Shift resizes, Delete removes). Each box has a card number (boxes sharing one are hidden together) and an optional label; an optional header (≤500) shows above the picture. Mode *Hide all* (`hideAll`) or *Hide one* (`hideOne`). `Occlusion.tsx`.
- **Schema (migration 0018).** `occlusions (id, asset_id, header, mode, boxes JSON, created_at, updated_at)`: never deleted; `asset_id` must be a kept picture and never changes. Boxes are 0–1 fractions of the picture, 1–200 of them, numbers 1–999, labels ≤200; Rust checks them (`occlusion::validate_occlusion`) and rounds to 4 decimals. Cards are `flashcards` rows with `occlusion_id`/`occlusion_number` set and `kind = 'basic'` (0012's CHECK only allows basic/cloze; widening it means rebuilding `flashcards`), so Rust tells them apart by `occlusion_id`. Triggers: both columns set or neither, fixed once written, one active card per (occlusion, number); 0010/0012's `flashcards_deleted_are_read_only` recreated with the two columns.
- **Fronts and backs stored** as for cloze: front `header\n[picture: box N]`, back the labels joined by "; " (or `[box N]`). Lists, search, counts and statistics work unchanged.
- **Editing** (`update_occlusion`) follows cloze's rules by number (keep / soft-delete / restore from the same deck / add). The picture can't change. `update_flashcard` refuses picture cards (`WrongKind`).
- **Restore, move, bury.** `occlusion::restore_block` (`numberGone`, `elsewhere`), `check_whole_occlusions` (`OcclusionSplit`), and `study::buried_cards!` keeps siblings apart for the day, all as cloze.
- **Review.** `SessionCard.occlusion` sends the picture, header, mode, number and boxes; the asked box shows `?`, revealed shows its label. Help with a card isn't offered (`offerHelp` false).
- **CSV export leaves picture cards out** (a spreadsheet can't carry the picture).
- **Anki** (`anki.rs`). A note type whose template has `image-occlusion` becomes picture cards: `{{cN::image-occlusion:rect:left=…:top=…:width=…:height=…:oi=1}}`; ellipses and polygons become their bounding box, angles are dropped, text shapes skipped (counted as `occlusionApproximated`); `oi=1` → hideAll. Shapes in pixels, or a picture missing from the package, leave the note out (`skipped.imageOcclusion`). The picture is read from the media map (legacy JSON, or the newer zstd protobuf `MediaEntries`, files zstd-compressed) and kept with `assets::put`, once per package picture. Fixtures `occlusion.apkg` and `occlusion-legacy.apkg` (made by `make_fixtures.py` with Anki 25.09).
- **Not done.** Rectangles only (no freehand or polygons); no picture swap; not checked on Windows or in the real app window.


## 4.16 The Sources screen and page links

Released in v0.8.0 (written on branch `claude/sources-screen-9yz936`): phase 2 of "Sources and pictures". No migration (0019 left unused).

- **Sources screen** (`Sources.tsx`). Lists kept documents (kind, pages, size, date) and deleted ones with *Restore*; *Add source*. A source shows its text page by page (previous/next, jump), the notes written from it, and *Make a note*, *Open original*, *Save a copy*, *Rename*, *Delete* (soft, asks first).
- **Import file keeps the original.** *Notes* → *Import file* now calls `import_source`: the file is kept as a source as soon as it's chosen, and *Save note* calls `create_note_from_source`, which writes the note, its pictures (since the merge with 4.14) and its `source_notes` link in one transaction (`notes::insert_note` on the caller's connection). A note from a source links back to it, or says it was deleted.
- **Page links.** `sources::locate_quotes` / `quoted_page` find a suggested card's or quiz question's quote on the source's pages with `suggest::quoted_from`'s word-for-word rule ("…" gaps allowed, across a page break). "From page 7 of …" opens `SourcePageDialog` with the quote marked (`SourcePages.tsx`). Cards still store no source: the link lives only while drafts or the quiz are open.
- **Originals.** `open_source_original` copies the kept file to `<temp>/synapse-originals/<hash prefix>/<name>` and opens it with the opener plugin; `save_source_copy` writes a copy where the user chooses. Both check the file's hash first (`sources::original`); the kept file is never handed out.
- **Not checked** in the real window, on Windows, or *Open original* on a real desktop.

## 4.17 Text recognition (foundation)

Written on branch `claude/project-thread-50ujao` (not yet released): the shared foundation of the "Text recognition" major feature. Sections 4.18–4.20 are the screens built on it; the contract for the pieces built on it is `/mnt/project-files/planning/text-recognition-contract.md` (project files).

- **Engine** (`ocr.rs`). [ocrs](https://github.com/robertknight/ocrs) 0.13, pure Rust on `rten`; its two models (`src-tauri/models/`, CC BY-SA 4.0, unchanged, hashes checked by a test) are compiled in with `include_bytes!`, so no DLL, resource or download. About half a second for a page on a laptop-class CPU. Reads the Latin alphabet without accents ("è" → "e", some → "?") and handwriting poorly: Samu chose (2026-10-21) the built-in engine plus an optional local vision model in Ollama for accents and handwriting.
- **Scanned PDF pages.** The largest image a page draws (through forms too), decoded in Rust: DCT (JPEG), raw/Flate/LZW 8-bit grey, RGB, CMYK (ICCBased by its N), 1-bit and image masks, CCITT G4 and G3 1-D (`fax` crate); `/Rotate` applied. JPX and JBIG2 are `unreadablePicture`. No page rendering: vector text on a page is extraction's job.
- **Helper process.** Synapse started with `--recognise-text` (`main.rs`): a JSON job line then the bytes on stdin, one JSON line per page on stdout. A crash, or 90 s on one page (`PAGE_TIMEOUT`), is a failed recognition; stopping kills the child. Pictures over 80 MP are refused before decoding; longer sides over 4,000 px are scaled down.
- **Schema (migration 0019).** `recognitions (asset_id, page, engine 'ocrs'|'ollama', model, text ≤100,000, lines JSON, width, height, created_at)`. Page 0 for a picture, 1+ for a PDF (trigger); never updated or deleted; the newest row per (asset, page) is in use. By file, not by note/card/source, so equal files are read once. Backups carry it unchanged (format 2 still); an older Synapse refuses a schema-19 backup as newer.
- **Sources.** `source_pages` stays as extraction read it; `SourcePage.recognised` offers the page's recognised text beside it, and `readable()` (extracted text, or recognised where blank) is what `locate_quotes`/`quoted_page` search. `import_source` now keeps a PDF with no text at all (body empty, every page in `pagesWithoutText`); `import_file` still refuses it.
- **Commands.** `recognise_picture` (cached: a picture read by today's models isn't read again), `get_picture_recognition`, `get_pages_to_recognise`, `recognise_source_pages` (pages or all unread blank pages; a `Channel` of `{page, done, total, problem}`; each page saved as read; stop keeps them), `stop_recognition` (`Recognising` state, one at a time).
- **Text.** Lines in reading order; a blank line where a gap is over 1.6× the usual one; lines in a paragraph joined as an imported PDF's are (`extract::unwrap_lines`).
- **Not done here:** searching recognised text, OCR of text drawn as vector shapes, languages beyond unaccented Latin. Not checked on Windows.

## 4.18 Scanned pages on the Sources screen

Written on branch `claude/project-thread-sh8b9z` (not yet released), on top of 4.17: the first screen that uses text recognition. Frontend only (`ScannedPages.tsx`, `Sources.tsx`, `Notes.tsx`); no command, no migration.

- **Read scanned pages (N).** A PDF source with pages `get_pages_to_recognise` lists offers to read them all, with "Reading page 3 of 12…", a bar and *Stop* (`stop_recognition`); leaving the screen stops it too. Then a summary: pages read, pages where nothing was found, and pages skipped with a plain reason per `PageProblem`. Skipped pages aren't offered again on that screen; a page that draws no picture is kept by Rust as an empty reading, so it isn't offered on later visits either, while a picture Synapse couldn't decode is.
- **The page viewer.** A page with no text of its own shows its recognised text, set apart, under "Recognised text, check for mistakes", saying which engine read it and when (and, for the built-in one, that it doesn't read accents). *Read this page* / *Read this page again* reads one page (`pages: [n]`). The page dialog of a quote's link carries the same label.
- **Notes.** `draftFromPages` uses `readable` text, `pagesWithoutText` counts only pages still without text, and the new-note form says which pages were recognised.
- **Importing a scan.** When *Import file* keeps a PDF with no text at all, a step before the note form offers *Read scanned pages*, *Write the note myself* or *Cancel*; after reading, the form opens with the text. A scan whose file was read before (readings belong to the file) goes straight to the form. *Add source* opens the source with a notice pointing at *Read scanned pages*.

## 4.19 Text from pictures, and boxes for image occlusion

Written on branch `claude/project-thread-abrm8t` (not yet released), on top of 4.17. Frontend only: no command, no migration.

- **Text in picture** (`src/pictureText.tsx`: `usePictureReading`, `PictureTextPanel`). On each picture in the note view's gallery and in the note and card forms' picture fields (`PictureGallery readText`, `PictureField addText`). The panel says "Recognised text: check it for mistakes" and which engine read it; *Copy* everywhere, *Add to note* / *Add to front* / *Add to back* in forms (appended after a blank line, counted as an unsaved edit). "No text found." for an empty reading. *Stop*, and leaving the screen stops it, but only if no newer reading has started (`newestRun`), since starting one stops the last.
- **Note from a photo** (Notes library). `choosePicture`, then `recognisePicture`; the new-note form opens with the text as the body, the picture attached and a notice to check it (`PhotoDraft`, `PhotoSummary`); no title is guessed. A photo with no text, or a reading stopped or refused, still opens the form with the picture.
- **Find labels** (`Occlusion.tsx`, `labelBoxes`). One box per recognised line, or per word; a line is split where a gap between words is wider than the line is tall (`labelRuns`), since the engine reads two labels side by side on a diagram as one line; padded by a fifth of the text's height (scaled by the picture's aspect ratio sideways), clamped to the picture, rounded to 4 decimals, labelled with the text cut to 200 characters, each a new card number. Text whose centre is already under a box with the same label is skipped. At most 200 boxes; the rest are counted in the message.
- **Checked** in the real app on Linux (Xvfb, scratch database): a typed-text photo read word for word; a diagram's four labels found (after the line split), plus one stray "S" read from a drawn circle, removed by hand; the boxes saved as four cards.
- **Not done:** no text in review pictures or in a deleted note's pictures; *Note from a photo* doesn't take a pasted picture.

## 4.20 Reading text with a local vision model

Written on branch `claude/project-thread-y4d3wh` (not yet released), on top of 4.17–4.19: the second engine Samu chose, for accents and handwriting. No migration (readings are `recognitions` rows with `engine = 'ollama'` and the model's name).

- **Models** (`vision.rs`, through `suggest.rs`'s `Ollama`: 127.0.0.1 only, no proxies or redirects). `get_vision_models` lists local models whose capabilities include `vision` (asking `/api/show` when the listing doesn't say); the chosen one is checked again before a picture is sent.
- **The picture.** The helper prepares it: a new `Job` returns a kept picture, or the picture a PDF page draws, as a PNG at most `ocr::MODEL_MAX_SIDE` (1,600 px) long, so PDF parsing stays out of the app. Sent through `/api/chat` with `images`, temperature 0, asking for a verbatim transcription; 10 minutes a page at most (`READ_TIMEOUT`).
- **Checking the answer** (`transcription`). Wrapping ("Here is the text:", code fences) is removed; an empty answer or one over 100,000 characters is refused; the model's "[no text]" is an empty reading. `agreement` gives the share of the built-in reading's words the model's text has (case and accents ignored; words it read with a "?" skipped) as a hint; the text is kept anyway.
- **Commands.** `get_vision_models`, `recognise_picture_with_model`, `get_pages_to_recognise_with_model`, `recognise_source_pages_with_model`; same `Recognising` state, so `stop_recognition` stops either engine. A page with no picture is stored as the same empty built-in reading as in 4.17.
- **Foundation change.** `recognise_picture`'s cache returns the built-in engine's own newest reading (`latest_by_builtin`), not the newest row, so *Find labels* keeps its boxes after a model has read the picture.
- **Frontend** (`ReadWith.tsx`). *Read with*: *Built in* or a vision model, remembered in web storage (`synapse.readWith`). Wired into *Read scanned pages*, *Read this page*, reading a scanned PDF on import, *Text in picture* (with *Read again* and the agreement shown) and *Note from a photo*. *Find labels* always uses the built-in engine (only it gives positions).
- **Checked** with Ollama 0.40.6 on CPU and qwen2.5vl:3b, in the real app on Linux with a scratch database: an Italian scanned page read with every accent right (about 4 minutes a page on CPU; 8 s warm in a direct call); a photo read again agreed 96% with the built-in reading. Not checked on Windows.

______________________________________________________________________

## 5. FSRS and personalization

### 5.1 What is already implemented

Synapse uses FSRS through the Rust implementation and currently uses default parameters.

Each rating contributes to:

- The card’s next scheduling state.
- Its next due date.
- Its persistent review log.

The scheduler is local. It is not a cloud AI call.

The original implementation boundary explicitly placed scheduling in Rust and required a durable review-log entry whenever the user rates a card.

### 5.2 What the stored state means

At a project level, the important concepts are:


| Concept | Role in Synapse |
| :-- | :-- |
| State | Whether a card is New, Learning, Review, or Relearning |
| Stability | Part of the memory model used for scheduling |
| Difficulty | Part of the memory model used for scheduling |
| Due | When the card should next be reviewed |
| Last review | When it was previously rated |
| Repetitions | Stored review-count information |
| Lapses | Stored forgetting-related count |
| Review log | Event history that records what happened |

Retrievability was planned as something computed from memory state and elapsed time rather than permanently stored as a fixed percentage.

### 5.3 Personalized parameters

Added after v0.6.0 (nightly round of 2026-10-19; not yet released). *Settings* → *Optimize* fits one global parameter set to every `review_logs` row with `fsrs::compute_parameters` (`optimizer.rs`).

- **Usable reviews** are ratings made at least a day after the card's previous rating (FSRS's training items). Optimizing is refused below 400 (`MINIMUM_REVIEWS`).
- **Nothing is written by optimizing.** The result is compared with the parameters in use (FSRS log loss decides; the screen shows the average prediction error and the intervals a new card would get) and held in memory under a token.
- **Applying** is only allowed when the fit predicts better, and is refused as stale if the parameters in use changed meanwhile. Each apply or *Reset to defaults* appends a row to `fsrs_parameters` (migration `0011`, rows never updated or deleted); the newest row is in use.
- **Future ratings only.** `study::record_review` reads the parameters in use for each rating. No stored schedule or review log is rewritten.
- **Bounded.** Fitting runs on a blocking thread; *Stop*, leaving the screen, or another run aborts it, and it stops after 120 seconds.
- **Not established:** the evaluation is on the same reviews the parameters were fitted to (as Anki does); no per-deck parameters, no automatic re-fitting. It hasn't been tried on a real long review history, only on generated ones in tests.

The older spec's policy (review-count doubling, daily cap) was not adopted: optimizing happens only when the user asks.

#### Desired retention

Added on branch `claude/nightly-2026-10-19-desired-retention-rcpjv1` (round 3 of 2026-10-19; not yet released). Before it, FSRS always aimed for 90%. *Settings* → *Desired retention* (`Retention.tsx`, `retention.rs`).

- **Values.** Whole percentages from 70 to 97 (`MIN_PERCENT`, `MAX_PERCENT`); 90 until changed. One global value, and an optional own value per active, ordinary deck, set on the Settings screen. The Sample deck and archived decks follow the global value; an archived deck's own value is kept and applies again once it's unarchived.
- **Stored append-only** in `desired_retention` (migration `0014`): `deck_id` NULL for the global value, `percent` NULL for a deck going back to the global value. Triggers refuse updates, deletes, and rows for the Sample deck, archived or missing decks. The newest row per scope is in use; the table's newest id is the version every change checks, so changes are refused as stale like `apply_parameters`.
- **Future ratings only.** `study::record_review` reads the card's deck value (`retention::for_deck`) for each rating and passes it to `scheduler::review`. No stored schedule or review log is rewritten. The optimizer's interval comparison uses the global value.
- **Estimate before applying.** *Estimate reviews* (`preview_retention`) runs `fsrs::simulate` over every active card in active decks, with the parameters in use, for 90 days, averaged over 4 fixed seeds, once at the values in use and once with the change. It shows reviews a day on average (first reviews included) and cards remembered on day 90. Ratings follow the user's own shares once there are 50 first ratings and 50 remembered reviews, else FSRS's averages. *Apply* appears only with an estimate. Nothing is written by estimating.
- **Not established:** the estimate assumes every due card is reviewed each day and no new cards are added, rounds intervals to whole days, and doesn't count same-day repeats after *Again* (Synapse can schedule those). It hasn't been compared with a real long review history.
- **Backups.** A backup made after 0014 has schema 14; an older Synapse (v0.7.0, schema 11) refuses it as newer than it knows. A v0.7.0 backup restores with 90% everywhere.

The older technical spec proposes an optimizer policy with a minimum review count, a review-count-doubling condition, and a daily cap. Those are project proposals, not a guarantee of appropriate behavior or a shipped capability. They should be revisited against the actual library and available data before implementation.

### 5.4 Scheduling is not LLM training

There are three separate ideas:

1. Updating a card’s schedule after a rating.
2. Fitting scheduling parameters using review history.
3. Training a language model.

Only the first is currently implemented.

The second is a planned statistical personalization feature.

The third is outside the intended project scope.

______________________________________________________________________

## 6. Current UI and navigation

### 6.1 The v0.3.1 shell

The current documented application has four main destinations:

1. Study Desk.
2. Notes.
3. Archived decks.
4. Backup \& restore.

The sidebar persists across screens and becomes a top navigation bar in narrower windows.

A location bar gives the current screen context.

### 6.2 Study Desk

The Study Desk is the entry point for active study.

Its documented behavior is:

- Put the first deck with due cards into a prominent focus card.
- Make Start review the primary action.
- List decks compactly with real due counts.
- Show an All caught up state when nothing is due.
- Provide access to notes.
- Show today's count under the intro: reviews recorded since local midnight and the distinct cards they were for ("Today: 12 reviews · 9 cards", or "No reviews yet today"). It counts every `review_logs` row of the day, including cards deleted or decks archived since. The day's bounds are worked out in Rust from the computer's time zone (`study::day_bounds`).

It is not currently a sophisticated recommendation engine. “The first deck with cards due” should not be described as “AI-selected best topic to study.”

### 6.3 Deck detail

The corrected UI uses:

- A page header with deck information.
- Counts.
- Start review and Add card actions.
- Compact card rows.
- Secondary management controls.
- Deleted-card recovery.


### 6.4 Notes

The current implementation provides a styled note library and note detail/edit flows.

Do not assume that every part of the Stitch concept—such as rich Markdown reading, linked-card counts, comparison blocks, or a permanent split-pane writing desk—has shipped. The current README still describes plain-text notes.

### 6.5 Review presentation

The redesign includes:

- A compact progress display.
- Separate question/answer presentation.
- Rating tiles.
- Number-key hints.
- Completion treatment.
- Consistent loading, error, and confirmation styling.


### 6.6 What happened during the design work

The first UI release, v0.3.0, adopted the general visual direction but did not reproduce the reference composition closely enough.

You noticed that the result looked like a restyled version of the old interface rather than the Study Desk you had designed.

v0.3.1 corrected the shell and layout consistency:

- Persistent navigation.
- A clearer Study Desk composition.
- Separate archived-deck and backup screens.
- More compact deck presentation.
- Shared styling across review, notes, recovery, and forms.

That sequence establishes a useful rule for later design work:

> Functional correctness and successful builds do not establish visual fidelity.

For a design-led feature, screenshot comparison and your approval are separate acceptance gates.

### 6.7 The discard prompt (v0.3.2)

The unsaved-changes question is a modal dialog built like the app's cards: a white rounded card over a dimmed screen. It has an amber warning icon, a display-font title, muted body text, and form-sized buttons. *Discard changes* uses the soft-red style of the delete confirmations, and *Keep editing* is a plain button.

Its first version reused the small amber inline-question style and looked out of place as a floating window. It was restyled before release at your request.

Focus lands on the dialog itself, without an outline, the way screens take focus, so a held Enter can't discard.

______________________________________________________________________

## 7. Technical architecture

The architecture is intentionally divided into presentation, command boundaries, business logic, and persistence.

```text
React + TypeScript
    Screens, forms, navigation, presentation
                  │
                  ▼
            src/api.ts
       Typed frontend API boundary
                  │
                  ▼
          Tauri commands
     Validated requests and responses
                  │
                  ▼
             Rust core
  Authoring, notes, sessions, scheduling,
      backup/restore, validation
                  │
                  ▼
          SQLx + SQLite
  Durable data, migrations, constraints,
             triggers
```

This is the central architectural rule: React presents the application; Rust owns the durable rules.

### 7.1 Frontend

Current README identifies these main files:


| File | Responsibility |
| :-- | :-- |
| `api.ts` | Typed calls into Rust, plus holding and closing the window |
| `Shell.tsx` | Shared sidebar and location bar |
| `Dashboard.tsx` | Study Desk |
| `DeckDetail.tsx` | Deck content and management |
| `ReviewSession.tsx` | Review interaction |
| `Notes.tsx` | Notes library and note flows, including importing a file as a note and searching the library |
| `markdown.ts`, `NoteMarkdown.tsx` | Reading a note's Markdown and showing it formatted, never as HTML (section 4.12) |
| `SuggestCards.tsx` | Suggesting cards from a note and checking them before saving |
| `Quiz.tsx` | A multiple-choice quiz on a note, and adding missed questions as cards |
| `Occlusion.tsx` | Picture cards (image occlusion): the form to draw boxes (and *Find labels*), and the picture shown in review |
| `pictureText.tsx` | Reading the text in a kept picture, and the panel that shows it as recognised text |
| `deckChoices.ts` | The decks a card can be added to, for both card-from-note screens |
| `ScannedPages.tsx` | Reading a PDF source's scanned pages: progress and Stop, the summary, and the label on recognised text |
| `ReadWith.tsx` | Choosing the reader (built in or a local vision model in Ollama), remembered per computer, and the agreement hint |
| `Search.tsx` | Search: every card and note at once, with snippets and filters |
| `Statistics.tsx` | Statistics: reviews per day, true retention, cards by state, due forecast (plain SVG charts) |
| `ArchivedDecks.tsx` | Archived-deck recovery |
| `Backup.tsx` | Export and restore |
| `Settings.tsx` | Settings: personal FSRS parameters (optimize, compare, apply, reset) |
| `Retention.tsx` | Settings: desired retention for all decks and per deck, with an estimate before applying |
| `CardsCsv.tsx` | Importing cards from a CSV/TSV file into a deck and exporting a deck's cards, on the Backup & restore screen |
| `AnkiImport.tsx` | Importing an Anki package as new decks, on the Backup & restore screen |
| `App.css` | Shared visual tokens and presentation |
| `forms.tsx` | Shared form fields and saving (one save at a time, field errors) |
| `unsaved.tsx` | Tracks unsaved forms and asks before discarding them |
| `*.test.tsx`, `src/test/` | Frontend tests and the fake Rust backend they run against |

The frontend should not duplicate durable scheduling or restoration rules.

For example, hiding a Restore button is useful UX, but the Rust command must still reject an invalid restoration request.

### 7.2 Rust core

| Module | Responsibility |
| :-- | :-- |
| `main.rs` | Thin entry point |
| `lib.rs` | Application setup and command registration |
| `commands.rs` | Frontend-facing command layer |
| `db.rs` | Database location, opening, migration, seeding |
| `scheduler.rs` | FSRS scheduling calculations |
| `optimizer.rs` | Fitting FSRS parameters to the review history, and which parameters are in use |
| `retention.rs` | The desired retention, globally and per deck, and estimating its reviews with FSRS's simulator |
| `study.rs` | Deck study data, sessions, recording reviews, today's review count |
| `search.rs` | Read-only search of every active card and note through the FTS5 index |
| `stats.rs` | Read-only statistics: 30 days of reviews, true retention, cards by state, 14-day due forecast |
| `authoring.rs` | Deck/card creation and lifecycle |
| `notes.rs` | Note lifecycle and note search |
| `cloze.rs` | Cloze texts: parsing, writing their cards' fronts and backs, editing and restoring by number |
| `occlusion.rs` | Picture cards (image occlusion): checking boxes, fronts and backs, editing and restoring by number |
| `suggest.rs` | Asking Ollama for suggested cards, and checking them against the note |
| `quiz.rs` | Asking Ollama for a multiple-choice quiz on a note, and checking its questions against the note |
| `extract.rs` | Reading a file's text for import; the PDF helper process |
| `cards_csv.rs` | Reading and checking CSV/TSV files of cards, saving them all together, and writing a deck as CSV (formula cells protected) |
| `anki.rs` | Reading an Anki package's collection read-only and adding its front/back, cloze and image occlusion notes (with pictures) as new decks |
| `assets.rs` | Files kept by hash in `assets/` beside the database, and the `synapse:` scheme that shows them |
| `sources.rs` | Imported documents kept as sources, with each page's text, and the note saved from each |
| `ocr.rs` | The built-in text recognition engine, the scanned picture on a PDF page, and the `--recognise-text` helper process |
| `recognition.rs` | What was read from each kept picture or PDF page, and reading them through the helper (stoppable) |
| `vision.rs` | Reading a picture or a source's pages with a local vision model in Ollama, and checking its answer |
| `export.rs` | Backup creation (format 2: the database and every kept file) |
| `import.rs` | Backup checking and replacement, kept files included |

The original design also deliberately separates scheduling calculation from database access. That helps make it possible to test scheduling behavior without launching the desktop interface.

### 7.3 Command contracts

A command contract includes:

- The command name.
- Argument names and types.
- Result shape.
- Field naming conventions.
- Error categories.
- State preconditions.
- What is allowed to change.

Changing a result shape can break the frontend even if Rust still compiles.

Changing frontend assumptions can produce subtle bugs even if TypeScript passes.

That is why UI-only work repeatedly had a boundary around existing Tauri contracts.

### 7.4 Error behavior

Your implementation reports use distinctions such as:

- Invalid input or missing object.
- Stale state.
- Operational/database failure.

Stale state is particularly useful for reversible lifecycle actions. If a note was already restored, a second restore is not treated as a new write. The interface can reload and explain that the screen was out of date.

User-facing errors should remain short and safe. Detailed SQL and local paths belong in development diagnostics rather than ordinary error messages.

______________________________________________________________________

## 8. Data model

The original specification documents the core tables; later implementation reports add deletion/restoration behavior. The current README confirms that migrations extend through `0010`.

### 8.1 `decks`

Stores the deck’s identity and lifecycle:

- ID.
- Name.
- Description.
- Sample flag.
- Creation time.
- Archive time.

A deck is active when its archive field is empty.

### 8.2 `flashcards`

Stores content and current learning state:

- ID and deck ID.
- Front and back.
- FSRS stability and difficulty.
- FSRS state.
- Due date.
- Last review.
- Repetition and lapse counts.
- Creation/update timestamps.
- Deletion time.


### 8.3 `review_logs`

Stores review events, including fields such as:

- Card ID.
- Rating.
- State before the review.
- Scheduled interval information.
- Elapsed interval information.
- Stability and difficulty after the review.
- Review timestamp.

This event history is distinct from the card’s current state.

### 8.4 `sessions`

Stores:

- Session ID.
- Deck ID.
- Start time.
- End time.
- Number of reviews recorded in the session.

The design permits at most one unfinished session per deck.

### 8.5 `notes`

Stores:

- ID.
- Title.
- Body.
- Creation time.
- Last-edit time.
- Deletion time added by the note lifecycle work.

Notes remain global rather than deck-scoped.

### 8.6 `seed_markers`

Stores whether a one-time seed operation has already happened.

This protects against a familiar bug: sample content reappearing because the application assumes “no sample card exists” means “the user has never launched the app.”

### 8.7 Future tables are not current tables

The old spec discusses settings, memory aids, model-usage logs, optimizer runs, and a pending-review queue.

Those are planning concepts. Their appearance in the specification does not establish that they exist in the released database.

In particular, a future queue must not weaken the current requirement that accepted reviews be durably recorded.

______________________________________________________________________

## 9. Database migrations and invariants

### 9.1 Migration sequence

The project history gives this high-level progression:


| Migration | Main purpose |
| :-- | :-- |
| `0001` | Initial flashcards |
| `0002` | Scheduling state and review logs |
| `0003` | Decks and sessions |
| `0004` | Authoring validation and naming constraints |
| `0005` | Card soft deletion and seed marker |
| `0006` | Deck archiving |
| `0007` | Typed notes |
| `0008` | Note soft deletion/restoration |
| `0009` | Deck unarchive |
| `0010` | Card restore |
| `0011` | FSRS parameters fitted to the user's history (append-only) |
| `0012` | Cloze cards: `cloze_texts`, and `kind`/`cloze_text_id`/`cloze_number` on `flashcards` |
| `0013` | Full-text search index (FTS5) of cards and notes, kept by triggers |
| `0014` | Desired retention, global and per deck (append-only) |
| `0015` | Indexes on `review_logs` by card and by time (speed only; no rule or row changes) |
| `0016` | Kept files (`assets`), sources with their pages, and the note saved from each (branch `claude/sources-foundation-yugkjq`; section 4.13) |
| `0017` | Pictures in notes and on card sides (`note_pictures`, `card_pictures`; branch `claude/sources-pictures-04zgws`; section 4.14) |
| `0018` | Image occlusion: `occlusions`, and `occlusion_id`/`occlusion_number` on `flashcards` (branch `claude/sources-image-occlusion-8pmk5r`; section 4.15) |
| `0019` | Text recognition: `recognitions`, what was read from each kept picture or PDF page (branch `claude/project-thread-50ujao`; section 4.17) |

The first seven are described in the original specification. The final three are established by your later feature reports and repository history.

### 9.2 Forward-only policy

Applied migrations should not be edited.

Instead:

1. Add a new migration.
2. Preserve existing data.
3. Adjust constraints or triggers explicitly.
4. Test fresh databases.
5. Test upgrades.
6. Test restoring older backups.

This is especially important because the backup validation contract checks migration history and schema consistency.

### 9.3 Why triggers exist

The project does not rely solely on a button being disabled.

Database-level protections help enforce rules such as:

- Notes/cards/decks are not hard-deleted through normal supported operations.
- A deleted item cannot be arbitrarily rewritten.
- Restoring clears only the relevant lifecycle state.
- Archived decks cannot receive ordinary card/session mutations.
- Protected sample behavior remains intact where enforced.
- Invalid empty text is rejected.

The restore migrations narrowed earlier “final state” protections so restoration became possible without allowing unrelated edits.

### 9.4 Preserve the row, change the state

A recurring pattern is:

```text
Delete note     → set deleted_at
Restore note    → clear deleted_at

Archive deck    → set archived_at
Unarchive deck  → clear archived_at

Delete card     → set deleted_at
Restore card    → clear deleted_at
```

The simplicity of that description hides the important work: transactional state checks, stale handling, trigger compatibility, and preservation tests.

______________________________________________________________________

## 10. Backup, restore, and portability

This is one of the most developed parts of the current application.

### 10.1 Current backup format

A backup contains exactly:

```text
manifest.json
db/synapse.sqlite
assets/<sha256>     (format 2: one per row of `assets`, none if there are none)
```

The manifest identifies the format, schema/app versions, export time, the database entry, and (format 2) the `assets/` folder. Format 2 comes with migration 0016 (section 4.13); format 1 backups (v0.7.3 and older) still restore, and v0.7.3 refuses format 2 as too new.

The database snapshot carries the complete stored study state, including deleted and archived content.

### 10.2 Export behavior

The documented design uses SQLite `VACUUM INTO` to create a consistent snapshot rather than simply copying an active database file.

The user chooses a destination through a native dialog.

Temporary output is prepared before the completed archive replaces the destination.

### 10.3 Import is replacement, not merging

This distinction should always be visible:

> Restore replaces the current study environment with the backup’s environment.

It does not:

- Append a deck.
- Merge notes.
- Import an Anki collection.
- Import CSV.
- Accept any arbitrary SQLite database.
- Let the user pick a subset of records.


### 10.4 Validation before replacement

The documented restore design checks:

- Archive structure and entry names.
- Manifest identity and supported format.
- Size limits.
- Database header.
- Integrity.
- Migration history.
- Schema.
- Compatibility.
- Foreign-key consistency after upgrading an older backup.

It prepares a checked copy before confirmation.

### 10.5 Replacement safety

The design includes:

1. Locking access.
2. Closing the current connection pool.
3. Creating a rollback copy.
4. Swapping in the checked database.
5. Reopening and validating it.
6. Restoring the rollback copy if the new database fails to open correctly.

The current README also discloses that an interrupted restore can leave a rollback copy requiring manual recovery.

### 10.6 Future attachment storage changes this contract

Once Synapse preserves PDFs, audio, or images, the database alone is no longer the entire environment unless those assets are stored inside it.

That creates an architectural decision:

- Store binary assets in SQLite.
- Store managed files alongside SQLite and package them in exports.
- Or deliberately ship a smaller import-as-text feature that does not retain originals.

The original vision favors preserving source material and extracted text, while the proposed first PDF slice in this conversation discarded originals after extracting text. Those are different product scopes and should not be silently conflated.

______________________________________________________________________

## 11. What is not implemented

The current README is explicit about these gaps.

### Content and ingestion

- Keeping originals of anything but imported documents (v0.8.0 keeps imported documents as sources, section 4.16, and pictures in notes and on cards, section 4.14).
- Pictures in CSV, pictures from Anki notes other than image occlusion, and dropping files into the window.
- OCR.
- Handwritten-note recognition.
- Audio transcription.
- YouTube transcript import.
- Web clipping.
- Rich-text formatting, or Markdown while editing. Markdown is shown formatted when reading a note (section 4.12).


### AI

- Flashcard generation beyond v0.4.0's local suggestions: no cloud providers, no choice of card count, and no suggestions from notes over 12,000 characters.
- Summaries.
- Quizzes beyond multiple choice on one note (section 4.9).
- Memory aids beyond help with a card after repeated Agains (section 4.8).
- Feynman feedback.
- Socratic tutoring.
- Local-model setup inside Synapse (Ollama is installed and managed by the user).
- Cloud-provider integrations.
- Local/cloud routing.


### Organization

- Fuzzy or meaning-based search. *Search* (section 4.10) matches word prefixes across all active cards and notes; within one deck, cards can also be filtered by exact text (section 4.2), and notes by exact text.
- Tags.
- Folders.
- Bulk actions beyond deleting, moving and restoring a deck's selected cards (and *Restore all*).
- Card movement between decks through restoration.
- External deck formats beyond CSV/TSV and Anki packages (Anki: front/back and cloze text only, section 4.7b).


### Analytics and personalization

- User-facing statistics beyond the Study Desk's count of today's reviews and the Statistics screen (30 days of reviews, true retention, cards by state, 14-day due forecast).
- Streaks.
- XP.
- Mastery percentages.
- Per-deck FSRS parameters or automatic re-fitting (one global set, fitted on request since the 2026-10-19 round). Desired retention can be set globally and per deck (section 5.3).


### UX and testing

- Drafts or autosave. Since v0.3.2 unsaved typing is protected by a prompt, but nothing survives a restart.
- Frontend tests for layout, visuals, or the real desktop window. The v0.3.2 suite covers interactions only.


### Distribution

- The current release installers are unsigned.
- The verified release artifacts are Windows x64; this is not evidence of tested macOS/Linux releases.

These are limitations, not hidden failures. They define the boundary of the released product.

______________________________________________________________________

## 12. Quality and testing

### 12.1 Current validation layers

The workflow has used:

```text
Rust formatting
      ↓
Rust unit/integration tests
      ↓
Clippy with warnings denied
      ↓
TypeScript checking + frontend build
      ↓
Frontend tests (Vitest)
      ↓
Isolated native application smoke test
      ↓
Read-only review
      ↓
Release artifact checks
```

Common commands include:

```powershell
# From src-tauri/
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --all-features -- -D warnings

# From the repository root
npm run build
npm test
```

Exact command choices should follow the current project configuration rather than a stale prompt.

### 12.2 Reported test progression

Your reports recorded:

- 99 tests before note restoration.
- 109 after note deletion/restoration.
- 119 after deck unarchive.
- 129 after card restoration.
- 133 during the broader preservation/upgrade verification.
- 136 after session progress and keyboard ratings.
- 136 Rust tests at v0.3.2, passing on Linux and in the Windows release run.
- Frontend: 16 tests with the unsaved-changes prompt, then 59 with the Stage B suite (v0.3.2).
- v0.4.0: 160 Rust tests plus 1 skipped real-Ollama test, and 79 frontend tests, passing on Linux and in the Windows release run.
- v0.5.0: 175 Rust unit tests plus 1 skipped, 4 helper-process integration tests, and 88 frontend tests, passing on Linux and in the Windows release run.

The totals up to v0.3.1 are historical reports. The v0.3.2 totals were run in a cloud session and in the Windows release run, not in your local environment.

### 12.3 The tests protect meaningful invariants

The strongest coverage described in the project history includes:

- Upgrading a database created by an older version.
- Preserving every relevant row during migration.
- Restoring old backups and then recovering deleted content.
- Keeping review logs and schedules intact.
- Refusing arbitrary changes to deleted/archived records.
- Avoiding cross-deck leakage.
- Persisting through close/reopen.
- Ensuring rejected operations write nothing.

That is more valuable than simply counting tests.

### 12.4 Frontend tests and what they don't cover

Since v0.3.2 the React side has a test runner: Vitest with jsdom and Testing Library, run with `npm test`. The 59 tests drive the real screens through `api.ts` against a fake Rust backend (`src/test/backend.ts`, built on `@tauri-apps/api/mocks`). They cover:

- Reviewing: reveal before rating, Space/Enter and 1–4, ignored modifier and held keys, one rating per press while saving, failed and stale ratings, progress and the summary.
- Confirmations and where focus goes, for deleting, restoring, archiving and unarchiving.
- Stale refusals that reload and explain, and repeated presses that send once.
- Field errors that take focus, and the card-from-note deck refusal.
- The sidebar locking during backup checking, confirming, restoring and exporting.
- The unsaved-changes prompt for every form, including the window close.

Each guard they cover was checked by breaking it and seeing a test fail.

What they still don't establish:

- Visual fidelity and layout. Screenshots and your approval remain separate acceptance gates.
- The real desktop window. jsdom has no modal dialogs (`src/test/setup.ts` stands in for them), and Tauri is mocked.
- Agreement with Rust. The fake backend answers as the tests say, so if a Rust command's contract changes, these tests won't notice.

### 12.5 Running checks in a cloud session

A Claude Code cloud container starts without Tauri's Linux libraries, so the Rust side doesn't compile until they're installed. That has to happen in every session, unless it's added to the environment's setup script:

```bash
apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf build-essential libssl-dev libxdo-dev
```

With them, `cargo fmt --check`, clippy and `cargo test --locked` run as on Windows.

The real app can also be run under Xvfb with a scratch `SYNAPSE_DB_DIR` and driven with `xdotool`. That's how the v0.3.2 window-close behaviour was checked before release. It runs on WebKitGTK, not WebView2, so it doesn't replace a Windows check.

______________________________________________________________________

## 13. Version history

| Version | Main role |
| :-- | :-- |
| v0.1.0 | Initial packaged offline study application |
| v0.1.1 | Note soft deletion and restore |
| v0.1.2 | Deck unarchive |
| v0.1.3 | Deleted-card restoration |
| v0.2.0 | Reliability, upgrade, and restore-preservation verification |
| v0.3.0 | Redesigned presentation plus the accumulated review-experience work |
| v0.3.1 | Corrective Study Desk shell and visual alignment |
| v0.3.2 | Unsaved-changes protection and the first frontend test suite; first release built by the Release workflow |
| v0.4.0 | Cards suggested from a note by a local model in Ollama, kept to the note by verified quotes |
| v0.5.0 | Importing PDF, text, Markdown, Word, and PowerPoint files as text-only notes |
| v0.5.1 | Suggested cards from long notes, one part of up to 12,000 characters at a time |
| v0.6.0 | Searching notes, *Restore all* deleted cards of a deck, today's reviews on the Study Desk |
| v0.7.0 | Personal FSRS parameters (0011), statistics, bulk card actions, CSV cards, help with hard cards |
| v0.7.1 | Cloze cards (0012), search everywhere (0013), desired retention (0014), note quiz, decks from Anki, CSV formula protection |
| v0.7.2 | Cloze siblings kept apart for the day, Anki cloze notes imported, notes read as formatted Markdown |
| v0.7.3 | Speed on long review histories: `review_logs` indexes (0015), next card without sorting the deck, lighter deck checks and Settings count |
| v0.8.0 | Sources and pictures: kept files and sources (0016), backup format 2, the Sources screen with page links, pictures in notes and on cards (0017), image occlusion cards and Anki image occlusion import (0018) |
| v0.8.1 | Restore selected deleted cards, and read a deleted note without restoring it |

Not every feature was committed at the same time as its release bump. Session progress and keyboard ratings were committed September 22 and packaged later.

### Release-process improvement

For future versions, I recommend this order:

1. Implement.
2. Test and review.
3. Obtain visual/product acceptance where relevant.
4. Bump versions.
5. Build release installers.
6. Verify the final diff and artifact versions.
7. Commit the exact release tree.
8. Push.
9. Tag that commit.
10. Publish the matching artifacts.
11. Verify uploaded assets and tag resolution.

This is preferable to publishing a tag before discovering whether installer bundling succeeds.

The five version-bearing files in your established workflow are:

- `package.json`
- `package-lock.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`
- `src-tauri/tauri.conf.json`

### The Release workflow (since v0.3.2)

`.github/workflows/release.yml` carries out steps 5 and 9–10 of that order on GitHub's Windows machines. It runs only when started by hand (Actions → Release → Run workflow) on `main`. In order, it:

1. reads the version from `tauri.conf.json`, and checks that `package.json`, both versions in `package-lock.json`, and `Cargo.toml` agree (`cargo test --locked` covers `Cargo.lock`);
2. refuses a version whose tag already exists;
3. runs `npm test`, `cargo fmt --check`, clippy with warnings denied, and `cargo test --locked`;
4. builds the MSI and NSIS installers, checks both exist, and logs their sizes and SHA-256 hashes;
5. only then creates the tag on the run's commit and publishes the release, with `.github/release-notes.md` as its notes.

So a release is:

1. bump the five version files;
2. rewrite `.github/release-notes.md`;
3. merge;
4. run the workflow;
5. check the published assets against the hashes in the run log.

Things to know:

- A run that fails before the last step builds, tags and publishes nothing, so it can be fixed and run again.
- v0.3.2's first run stopped at the version check, on a PowerShell quirk: `package-lock.json` has an empty key, which needs `ConvertFrom-Json -AsHashtable`. The fix went into the commit that was then released.
- v0.5.0's first run stopped at Clippy. The workflow installs the latest stable Rust (1.99 then), whose clippy had a lint (`chunks_exact_to_as_chunks`) that the 1.97 used locally didn't. The fix (#11) is the commit that was released. Since then `rust-toolchain.toml` pins the toolchain (1.99.1) for local builds and both workflows, so a new stable release can't change the lints. Moving to a newer Rust is a deliberate edit to that file.
- The workflow creates a lightweight tag. v0.3.0 and v0.3.1 were annotated tags.
- `actions/checkout` and `actions/setup-node` are on v5 (v4 targeted Node 20).
- A **Checks** workflow (`.github/workflows/checks.yml`) runs the same checks, plus `npm run build`, on every pull request, on `windows-latest` with the pinned toolchain. It builds no installers.
- The installers are still unsigned.

______________________________________________________________________

## 14. Full intended product roadmap

The original plan is larger than flashcards. It can be organized into five capability layers.

### 14.1 Material ingestion

Planned inputs:

- PDFs.
- Slides.
- Typed notes.
- Scanned pages.
- Handwritten notes.
- Lecture audio.
- YouTube transcripts.
- Explicitly selected webpages.
- Photographed book pages.

The guiding requirements are:

- User-selected material only.
- Inspectable extracted content.
- Honest extraction limitations.
- Local processing where practical.
- No automatic unrelated research.
- Source preservation where the product has committed to it.


### 14.2 Study-resource generation

Planned outputs:

- Flashcards.
- Summaries at different depths.
- Multiple-choice questions.
- Short-answer questions.
- Cloze questions (first form, typed cloze cards, on the 2026-10-19 branch; section 4.11).
- Applied questions.
- Memory aids.

These should be reviewed as learning resources, not treated as automatically correct because a model produced them.

### 14.3 Alternative study modes

Planned modes include:

- Active recall—the current foundation.
- Feynman explanation: the user explains an idea and receives feedback.
- Socratic tutoring: questions guide reasoning rather than immediately supplying answers.
- Push-to-talk voice review.

Voice is intentionally its own later phase because it combines transcription, reasoning, speech output, device handling, and interaction state.

### 14.4 Memory aids for repeated difficulty

The product brief proposes a suggestion after three consecutive failures by default, with user-adjustable sensitivity.

The user chooses among:

- A mnemonic.
- A simpler explanation.
- An analogy.
- A reframed question.

The user should also be able to request help manually.

Before implementation, the team must define what “failure” means in the actual rating model and whether consecutive failures are per card, per review event, or across sessions. The brief gives the intended experience, not every operational rule.

### 14.5 Local and cloud processing

The intended model is user control with sensible recommendations:

- Local preferred where practical.
- Cloud available as an explicit enhancement.
- No silent switching.
- Clear unavailability states.
- Visible provider/model choice.
- Honest disclosure of what content leaves the device.

The old spec proposes a provider abstraction and names `rig-core`, but that should be treated as a design candidate until implementation actually requires it.

______________________________________________________________________

## 15. The major architectural decisions still ahead

### 15.1 Source documents versus notes

You need to decide whether a source document is:

- Just text imported into a note.
- A first-class object with its own original file.
- A collection of extracted pages/chunks.
- A source that can create several notes or resources.

My recommendation is to keep “editable note” and “imported source” conceptually distinct if you intend to preserve originals.

Otherwise, editing a note can blur the difference between:

- What the source actually said.
- What extraction produced.
- What the student corrected.
- What an AI later summarized.


### 15.2 Provenance

Current cards are self-contained and do not link to notes.

Future AI generation can preserve that independence while still offering optional evidence or generation metadata.

You need not force a card to depend on a source document to let the user inspect why a generated answer was proposed.

The decision is not simply “links or no links”; it is whether links are:

- Required dependencies.
- Optional references.
- Draft-only evidence.
- Permanent generation metadata.


### 15.3 Draft generation versus direct writes

A safe first AI flow should be:

```text
Selected material
      ↓
Generate candidate cards
      ↓
Validate response structure and limits
      ↓
Show editable drafts
      ↓
User selects and approves
      ↓
Save ordinary cards
```

It should not be:

```text
Model response
      ↓
Immediately insert everything into the active review queue
```

This is a recommended next design, not a current feature.

### 15.4 Context limits and long inputs

Long documents need explicit policies:

- What is selected?
- What is omitted?
- How are pages or chunks identified?
- What happens when the model cannot accept everything?
- How is partial processing disclosed?
- How are duplicate suggestions handled?

A “Generate cards from PDF” button hides all of these questions.

### 15.5 Local model availability

A local model integration must handle:

- Service unavailable.
- No suitable model installed.
- Model unavailable.
- Request timeout.
- Cancellation.
- Invalid output.
- Resource constraints.
- A model ignoring instructions.
- A prompt containing malicious instructions embedded in source material.

Local execution is not equivalent to guaranteed correctness or safe input handling.

### 15.6 Cloud keys and backups

The intended policy is to keep provider credentials out of the portable database and backup package.

The old technical spec proposes OS credential storage for keys. That is a future design requirement, not a feature already present.

______________________________________________________________________

## 16. Recommended next development sequence

You asked for the whole project rather than a task prompt, so this is a roadmap—not authorization for Claude to start all of it.

### Stage A — Close the most visible data-loss edge case

**Done in v0.3.2** (section 4.5): a prompt on sidebar navigation and window close. There is no autosave, and nothing survives a restart.

Add unsaved-changes protection.

Before v0.3.2, the README said navigating through the sidebar could discard an unsaved form.

A small feature should define:

- Which forms become dirty.
- Save / discard / stay behavior.
- How canceled navigation behaves.
- Whether window close is included.
- Whether drafts survive restart.

Do not conflate a navigation warning with full autosave. They are different features.

### Stage B — Add repeatable frontend regression coverage

**Done in v0.3.2** (section 12.4): 59 tests covering every item below.

Start with high-value interactions:

- Reveal before rating.
- Keyboard shortcuts.
- Duplicate-submit prevention.
- Confirmation focus.
- Restore and stale handling.
- Backup-flow navigation blocking.
- Unsaved-form navigation once implemented.

This supports faster changes without repeatedly depending on manual memory.

### Stage C — Choose the next capability leap

**Option 2 done in v0.4.0** (section 4.6). **Option 1 done, as text-only import, in v0.5.0** (section 4.7). Stage D is next: the 12,000-character suggestion limit is the first gap between them, since imported documents are often longer.

There are two reasonable paths.

#### Option 1: PDF ingestion first

Choose this if the immediate pain is manually moving study material into the app.

Bound the first release to text-based PDFs and be explicit about:

- Original-file retention.
- Page/text limits.
- Preview/editing.
- Scanned-document refusal.
- Partial extraction.
- Backup compatibility.


#### Option 2: Local generation from existing notes first

Choose this if the immediate goal is testing Synapse’s AI study value.

Existing notes avoid introducing document parsing and attachment storage at the same time.

The feature would be:

- Select an existing note.
- Request candidate front/back cards.
- Review and edit drafts.
- Choose a deck.
- Save approved cards.
- Continue using the app if the local model is unavailable.

My preference for testing the core AI product idea is generation from existing typed notes first. My preference for reducing material-entry friction is PDF ingestion first. Either is valid; combining both into the first AI release makes failures harder to diagnose.

### Stage D — Connect ingestion and generation

Once each works independently:

```text
Import material
      ↓
Inspect extraction
      ↓
Select content
      ↓
Generate drafts
      ↓
Approve cards
      ↓
Review with the existing FSRS loop
```

This is the first end-to-end version that truly demonstrates the larger Synapse concept.

### Stage E — Expand deliberately

Only then add:

- Summaries or one quiz format.
- Repeated-difficulty support.
- Optional cloud provider.
- More ingestion formats.
- Scheduling optimization.
- Voice.

Collaboration, mobile, and advanced subject-specific behavior should remain well behind a useful individual workflow.

______________________________________________________________________

## 17. How to describe the project honestly

### For the README today

> Synapse is a local-first desktop flashcard application built with Tauri, React, TypeScript, Rust, and SQLite. It supports manual decks and cards, typed notes, FSRS-based review sessions, reversible deletion and archiving, and portable backup/restore. It works offline without accounts or cloud services.

### For a portfolio

> I am building Synapse, a desktop study tool that keeps notes and review history on the user’s computer. I defined the product and architecture, used an AI-assisted development workflow, and validated data preservation across upgrades, restoration, and content recovery. The current release is an offline flashcard system; future work adds controlled ingestion and optional AI-assisted study-resource generation.

### For the long-term vision

> Synapse aims to turn a student’s own material into active learning experiences while preserving local ownership and making model use transparent.

### Claims to avoid for now

- “AI-powered flashcard generation.”
- “Upload any PDF.”
- “Learns your personal forgetting curve.”
- “Complete study-material ingestion.”
- “Supports local and cloud models.”
- “Fully tested.”
- “Production-ready with no race conditions.”
- “All data is encrypted.”
- “Works on every desktop platform.”

Some are planned. Some are not established. Some require an ongoing audit rather than a one-time test result.

______________________________________________________________________

## 18. What the project already demonstrates

Synapse is not merely a mockup.

It already demonstrates:

- A native desktop shell.
- A typed frontend/backend boundary.
- Rust-owned validation and business rules.
- SQLite persistence.
- Embedded forward-only migrations.
- A working scheduling integration.
- Durable review history.
- Reversible content lifecycle operations.
- Upgrade and backup compatibility work.
- Defensive restore behavior.
- A real installer/release pipeline.
- Iterative design correction after user review.

Its most distinctive engineering work so far is data preservation: recovery operations are intended to return stored content without silently resetting the user’s study history.

Its biggest remaining product gap is also clear: creating material is still predominantly manual. The larger vision becomes visible when Synapse helps transform existing material into good questions without compromising that trustworthy review foundation.

______________________________________________________________________

## 19. Project health assessment

| Dimension | Assessment |
| :-- | :-- |
| Offline manual study loop | Implemented and released |
| Deck/card/note lifecycles | Implemented, with recovery |
| Data preservation | Significant reported regression coverage |
| Backup portability | Implemented, with strict replace-only semantics |
| UI consistency | Improved substantially in v0.3.1 |
| Visual fidelity | Must still be judged against your reference in the real app |
| Frontend regression testing | Started in v0.3.2: 59 interaction tests; no visual or real-window tests |
| Unsaved work protection | Implemented in v0.3.2 (a prompt, not autosave) |
| Material ingestion | Text-only file import (PDF, TXT, MD, DOCX, PPTX) since v0.5.0 |
| AI study assistance | Local card suggestions from notes since v0.4.0, kept to the note by verified quotes |
| Public distribution | Windows releases built and published by the Release workflow |
| Signing and wider platform validation | Not established |
| Long-term vision | Defined, but much broader than the released app |

The right way to understand Synapse at this stage is as a completed offline study foundation with a deliberately unfinished intelligence layer. The foundation is valuable on its own; the next major challenge is to make bringing in material and producing good study questions easier without weakening correctness, privacy, or user control.
