Import files as notes: make a note from the text of a PDF, text (.txt), Markdown (.md), Word (.docx), or PowerPoint (.pptx) file. Existing study behavior, FSRS scheduling, data lifecycle rules, suggested cards, and the backup format are unchanged.

- In *Notes*, choose *Import file* and pick a file. Synapse reads its text and opens it as a new note, so you can check it and edit it before you save it. Nothing is saved until you do, and leaving or closing first asks whether to discard it.
- Only the text is kept. Synapse doesn't copy the file, link to it, or remember where it was: the note is an ordinary note, like one you typed.
- PDFs: each page's text, in order, with lines that were only broken at the edge of the page joined up again. Pages with no text are named so you can check nothing's missing. Scanned PDFs, whose pages are pictures of text, can't be read yet, and neither can password-protected ones.
- Word: the document's paragraphs, list items, and tables. PowerPoint: each slide's text in the order the slides are shown, with its speaker notes.
- Files up to 50 MB can be imported. A note holds up to 100,000 characters, so a longer text has to be shortened before it's saved; Synapse says when. Older .doc and .ppt files need saving in the newer format first.
- A damaged PDF can't close Synapse: it's read by a separate, short-lived copy of Synapse, and is simply reported as a file that couldn't be read.
- Suggested cards work on notes of up to 12,000 characters, so to suggest cards from part of a long document, put that part in a note of its own.

Windows x64 installers:

- Synapse_0.5.0_x64_en-US.msi
- Synapse_0.5.0_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
