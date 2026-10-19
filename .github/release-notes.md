Write cloze cards, search everything at once, choose how much you want to remember, take a quiz on a note, and bring in decks from Anki. Existing schedules and review history are unchanged, and backups from earlier versions restore as before.

- Cloze cards: *Add card* → *Cloze*, then write a text with gaps like `{{c1::mitochondria}}` or `{{c1::answer::hint}}`. Each gap number becomes its own card with its own schedule, and the form says how many cards the text makes. In review the gap shows as [...] (or the hint), and the answer is highlighted in place once revealed. Editing the text keeps the schedules of the gaps that stay; a gap you remove deletes its card, and a new one adds a card. A text's cards move between decks together.
- Search: a new *Search* screen finds words in every card and note at once, from the start of each word, ignoring capitals and accents, with the matching words highlighted. Show cards, notes, or both, and pick one deck for cards. Opening a result takes you to its card in its deck, or to the note. Deleted cards and notes, and archived decks, aren't searched.
- Desired retention: under *Settings*, choose how much FSRS aims for you to remember, from 70% to 97% (90% until now), for all decks or for one deck. Before applying, *Estimate reviews* shows roughly how many reviews a day the change would mean, simulated on your own cards; it's an estimate, not a promise. Only ratings made afterwards use the new value.
- Quiz on a note: next to *Suggest cards*, *Quiz me* has a model in Ollama, on your computer, write up to 8 multiple-choice questions from the note. A question is kept only if the passage it quotes is in the note word for word. Answer with the mouse or 1–4, see the quoted passage after each answer, and add the ones you missed to a deck as cards. Nothing else is saved.
- Decks from Anki: under *Backup & restore*, *Decks from Anki* reads an `.apkg` or `.colpkg` file, shows what it found, and adds each Anki deck as a new deck of front/back cards, with reversed cards if you tick the box. Cards start as new: Anki's schedule, review history, pictures and sounds aren't brought across, and cloze and other note types are left out and counted.
- Spreadsheets: an exported CSV cell that starts with `=`, `+`, `-`, `@`, a tab or a line break now gets a `'` in front, so a spreadsheet shows it as text instead of running it as a formula. Synapse takes that `'` off again when it imports its own files.
- Fixed: in 0.7.0, a missing line in the stylesheet left *Cards in a spreadsheet* (its preview table and check boxes) and *Get help with this card* without their layout. They now look as intended.

This release changes the database (migrations 0012 to 0014: cloze cards, the search index, and desired retention). A backup made with 0.7.1 can't be restored into 0.7.0 or earlier; backups from earlier versions restore into 0.7.1 as before.

Windows x64 installers:

- Synapse_0.7.1_x64_en-US.msi
- Synapse_0.7.1_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
