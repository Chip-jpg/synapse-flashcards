Synapse 0.8.0 is the first big feature release: **Sources and pictures**. Until now Synapse kept only text. An imported document became a note and the file was thrown away, and nothing could hold a picture. From 0.8.0 Synapse keeps files too: the documents you import, and pictures in notes and on cards. On top of that come picture cards (image occlusion), in Synapse and from Anki.

**Before you install, please read "Backups" and "Not tried yet" below.** This release changes the backup format.

## What's new

**Sources: your imported documents, kept as they were.**

- *Notes → Import file* still makes a note from a PDF, Word, PowerPoint, text or Markdown file. Now the original file is also kept, and it appears under **Sources**, a new place in the sidebar.
- A source shows the document's text page by page (or slide by slide). You can go to the previous or next page, or jump to a page. It also lists the notes you wrote from it.
- From a source you can:
  - *Make a note* from its text;
  - *Open original*, which opens a copy in your computer's own program for that kind of file;
  - *Save a copy* wherever you like;
  - *Rename* it;
  - *Delete* it. Delete asks first and can be undone with *Restore*.
- *Add source* keeps a document without writing a note.
- A note written from a source says so under its title and links back to it.
- Your note is still yours to edit freely; the source keeps what the document actually said.

**Suggested cards and quiz questions show their page.**

- When you ask Ollama to suggest cards or write a quiz from a note that came from a source, each card or question shows where its quote is, for example "From page 7 of Lecture 3".
- Clicking it opens that page with the quote highlighted, so you can check it against the original document.
- Saved cards don't remember the page. The link is there while you check the drafts or take the quiz.

**Pictures in notes and on cards.**

- A note can show up to 20 pictures under its text.
- Each side of a front/back card can show up to 4.
- In review, the front's pictures come with the question and the back's with the answer.
- Add them with *Add picture* (PNG, JPEG, GIF or WebP, up to 20 MB each). Move them earlier or remove them in the form; changing them counts as an unsaved change.
- Cloze cards don't take pictures.

**Picture cards (image occlusion).**

- *Add card* → *Picture* → choose a picture, then draw boxes over the parts to hide. A labelled diagram, a map or an anatomy figure works well.
- Each box is its own card with its own schedule, and it asks "what's under here?".
- Boxes can share a card number to be hidden together, and each box can have a label that is shown as the answer.
- *Hide all* covers every box on the question; *Hide one* covers only the one being asked.
- Boxes can be drawn, moved and resized with the mouse, or with the keyboard: arrow keys move a box, Shift with the arrows resizes it, and Delete removes it.
- Editing, deleting, restoring and moving picture cards work like cloze cards, including keeping a picture's cards apart on the same day.
- *Export CSV* leaves picture cards out, because a spreadsheet can't carry the picture.

**Anki image occlusion notes now come across.**

- *Decks from Anki* used to skip image occlusion notes. Now they become picture cards, with their picture kept by Synapse.
- This needs a package exported **with media included**.
- Rectangles come across as drawn. Ellipses and polygons become the box around them, rotated shapes come across unrotated, and text shapes are dropped; the preview counts the notes this affects.

## How files are kept

- Kept files live in an `assets` folder next to your database. Each one is named by a fingerprint of its contents (SHA-256), so the same picture used in ten places is stored once.
- Synapse checks that each file really is what it says (a picture must really be a picture; SVG is refused).
- Pictures are shown only from Synapse's own copy, never from an address in a note's text.
- A file is never changed or deleted once kept, even when the note, card or source that used it is deleted. So restoring a deleted item, or an older backup, always finds its files.
- The cost is that the folder only grows: Synapse doesn't clean up files nothing uses any more.

## Backups: please read

- **A backup now includes your kept files.** Backups use a new format (format 2): the database plus every kept file. Each file is checked when the backup is made and again when it's restored. If anything is missing or doesn't match, the backup or restore stops and nothing is changed.
- **Backups made with 0.8.0 can't be opened by 0.7.3 or older.** The older app refuses them as "from a newer version". If you might go back to an older Synapse, export a backup **before** updating and keep it.
- **Older backups still work in 0.8.0.** Backups from 0.7.3 and earlier restore as before; they simply have no kept files.
- This release adds three database migrations: 0016 (kept files and sources), 0017 (pictures in notes and on cards) and 0018 (picture cards). Your existing decks, cards, schedules, review history and notes are not changed by them.

## Not tried yet, and known limits

- **Nothing in this release has been tried on a real Windows computer yet.** It was built and tested by the Release workflow on GitHub's Windows machines, and tried by hand in the real app on Linux only. In particular, showing pictures, opening an original in your own program, and pasting pictures have not been seen working on Windows.
- **Pasting a picture doesn't work on Linux** (the WebKitGTK window doesn't pass the picture to the app); use *Add picture* there. On Windows, pasting is expected to work but hasn't been tried.
- **You can't drag files into the window.** Use *Add picture* or *Import file*.
- **Picture cards are rectangles only.** There's no freehand or polygon drawing, and a picture card's picture can't be swapped (make a new picture card instead).
- Pictures aren't exported to CSV, and pictures in Anki notes other than image occlusion aren't brought in.
- Kept files are limited to 64 MB each (20 MB for a picture), and to 2 GB and 60,000 files in all, so that a backup stays an ordinary zip file.

Windows x64 installers:

- Synapse_0.8.0_x64_en-US.msi
- Synapse_0.8.0_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
