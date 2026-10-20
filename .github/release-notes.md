Keep cloze cards from the same text apart, bring in Anki's cloze notes, and read notes written in Markdown formatted. Existing schedules, review history and the backup format are unchanged: backups made by 0.7.1 and 0.7.2 restore into each other.

- Cloze cards: once you rate one card of a cloze text, the other cards of that text wait until tomorrow, so the answer to one gap doesn't give away the next. Their schedules aren't changed, and the card you rated still comes back when it's due. The due counts on the Study Desk and the deck screen follow the same rule; the Statistics forecast and the retention estimate still count those cards.
- Decks from Anki: cloze notes now come across as cloze texts, one card per gap number, checked as a cloze text you type is. Their *Back Extra* isn't brought across, and the preview says how many had one. Image-occlusion notes, and cloze notes Synapse can't read (nested gaps, an empty answer, a number over 999), are still left out and counted.
- Notes: a note written in Markdown is shown formatted when you read it, with headings, emphasis, lists and task lists, quotes, code, and tables. *Formatted* / *Plain text* switches between the two, and Synapse remembers your choice. Links show their address and are never opened, and pictures aren't loaded. Editing is still plain text, and the note's text is stored exactly as you wrote it.

Windows x64 installers:

- Synapse_0.7.2_x64_en-US.msi
- Synapse_0.7.2_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
