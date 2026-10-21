Two small additions to getting things back after deleting them: bring back only the deleted cards you choose, and read a deleted note before deciding whether to restore it. Nothing about your cards, schedules, review history or backups changes, and backups made with 0.8.0 and 0.8.1 restore into each other.

## What's new

**Restore only the deleted cards you choose.**

- Until now, a deck's *Deleted cards* list could bring back one card at a time, or every deleted card of the deck at once with *Restore all*.
- Now each deleted card that can come back has a *Select* box. Tick the ones you want and choose *Restore selected*.
- Synapse asks first, saying how many cards it will restore, and then brings them all back together, or none of them if something goes wrong.
- Restored cards come back exactly as they were deleted: same deck, same schedule, same review history. A card whose due date has already passed is due straight away, as with any restore.
- If the list changed in the meantime (for example a card was already restored from another window), nothing is restored and the screen reloads so you can see where things stand.
- A cloze or picture card that can't come back yet has no *Select* box. That happens when its gap or box was removed from the text or picture, or when the other cards of the same text or picture now live in a different deck. These are the same rules as restoring a single card.
- There's no *Select all* for deleted cards yet; *Restore all* covers that case.

**Read a deleted note without restoring it.**

- Until now, a deleted note showed only its title and dates. To read what it said, you had to restore it first.
- Now *Deleted notes* has *Read note*. It opens the note read-only: its title, when it was written, edited and deleted, its text (formatted, if it's written in Markdown, just like an active note) and its pictures.
- The only action there is *Restore note*. Editing, writing a card from it, suggesting cards and quizzes still need the note restored first, and Synapse still refuses any change to a deleted note.

## Backups

No change. This release adds no migration: the database stays at schema 18 and backups stay in format 2, so backups made with 0.8.0 and 0.8.1 restore into each other.

## Not tried yet

Both features were checked by automated tests (271 for the screens, 359 for the Rust side), and reading a deleted note was also tried in the real app on Linux. Neither has been tried on Windows yet.

Windows x64 installers:

- Synapse_0.8.1_x64_en-US.msi
- Synapse_0.8.1_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
