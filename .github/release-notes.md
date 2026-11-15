This release brings Synapse into **Italian**, adds an **exam planner**, **lesson import** and a second round of the **local AI tutor**, lets you **search inside your sources**, and makes cards **remember the note they came from**. **It has two migrations (0021 and 0022), so backups made with 0.11.0 can't be opened by 0.10.0 or older** (see *Backups* below).

## What's new

**Italiano / English.**

- A new *Language / Lingua* setting switches the app between Italian and English. On first run Synapse follows your system language and remembers your choice.
- Translated in this release: the sidebar, Study Desk, reviews, Settings and desired retention, deck screen, search, statistics, exam planner, archived decks, backup and restore, CSV and Anki import/export, bulk card actions, picture cards (image occlusion) and the "unsaved changes" dialogs. Notes, sources, tags, quizzes and the tutor are still in English and will follow in the next release.
- Text recognition now gives common Italian words their accents back (*perché*, *città*, *più*, *così*, *università*…). The built-in reader still can't tell *e* from *è*. For every accent, use *Read with* a local vision model as before.

**Exam planner (verifiche e interrogazioni).** Give a deck or a tag an exam date. Synapse sets a daily goal so everything is seen before the date, shows a readiness score from FSRS's predicted recall, and adds a study streak and a calendar.

**Lessons in.** Import a lesson transcript (SRT, VTT, TXT or Markdown, e.g. from Dropscript) as a note with timestamps, or a saved web page as a note.

**AI tutor, round two (local Ollama only).** Note summaries, open-answer questions graded against your note, and an optional Socratic mode that asks follow-up questions. Nothing leaves your computer.

**Search inside Sources.** Search titles, file names, page text and recognised text, see snippets, and open the page with your words highlighted.

**Merge tags.** On the *Tags* screen, *Merge into…* moves one tag's cards and notes onto another in one safe step. Deleted or archived items keep the old tag.

**Cards remember their note.** Cards made from a note's suggestions, a quiz, the tutor or inside the note itself keep a link to it. After the answer is shown, *Show in note* opens it, and *Back to review* returns. Deleting the note hides the link, and restoring the note brings it back.

**Text in pictures, everywhere.** *Text in picture* (copy only) on deleted notes and on question/answer pictures during review. A note can also be made by pasting a photo.

**Look and feel.**

- *Theme*: System, Light or Dark. The dark theme covers every screen.
- The window now opens at 1200×800, centred (it was 800×600).
- **Ctrl+K** (Cmd+K on macOS) opens a command palette. Jump to any screen, open a deck, or start reviewing a deck that has cards due. Typing ignores accents and capitals.

**Security.** The app window now has a strict Content Security Policy: no inline or remote scripts, and network access only to Synapse's own backend.

## Backups

**There are two migrations: 0021 (exam dates) and 0022 (each card's source note).** Backups made with 0.11.0 can't be restored into 0.10.0 or older, which will say the backup is from a newer version. Backups from 0.10.0 and older still restore into 0.11.0; their cards just have no source note. The backup format itself hasn't changed.

## Pull requests in this release

#58, #59, #60, #65, #66, #70 (#67 exam planner, #68 lesson import, #69 AI tutor), #71, #72, #73, #74, #75, #76, #77, #78, #79, #80, #81.

## Not tried yet

- Everything was checked by automated tests: 443 for the screens plus the full Rust suite, on GitHub's Windows machines for every pull request.
- The Italian text was reviewed as written but hasn't been tried in the real app on Windows yet. Please report anything that reads oddly.

Windows x64 installers:

- Synapse_0.11.0_x64_en-US.msi
- Synapse_0.11.0_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
