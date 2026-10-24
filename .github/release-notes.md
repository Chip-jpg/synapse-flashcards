The big feature in this release is **tags**: short labels you give cards and notes so you can find them again across decks, filter them, and review them together. They work like Anki's tags, and tags in Anki packages now come in with the cards. **This release has a migration, so backups made with 0.10.0 can't be opened by 0.9.1 or older** (see *Backups* below).

## What's new

**What a tag is.**

- A tag is one word: `biology`, `exam-june`. Spaces and commas aren't allowed, because typing a space or a comma ends a tag. Use `-` to join words, or `::` for a tag inside another, like `bio::cells`.
- Upper and lower case don't matter: `Bio` and `bio` are the same tag, shown the way it was first written.
- A tag can be up to 60 characters long, and a card or note can have up to 20 tags.
- Every kind of card takes tags: front/back, cloze and picture cards. The sample deck doesn't.
- Deleting or archiving something keeps its tags, and they come back when you restore it.

**Tags on cards.**

- Every card form has a *Tags* field. Type a word and press space, comma or Enter to add it as a chip, and press Backspace to remove one. While you type, tags you already use are offered as buttons.
- On a cloze or picture card, the tags go on every card made from that text or picture.
- The deck's card list shows each card's tags, deleted cards included.
- *Show cards tagged* filters the deck to one tag. `bio` also matches `bio::cells`.
- With cards ticked, *Tag selected* adds tags to all of them or removes a tag from all of them in one step. The cards' text and schedules don't change.

**Tags on notes, and in search.**

- Notes get the same *Tags* field, including notes saved from a source. Tags show in the note, in the library and when reading a deleted note.
- The library can be filtered by tag.
- *Search* has a tag chooser. You can also type `#bio` in the search box: it lists everything tagged `bio` (and `bio::…`), or narrows your other search words to it.
- A new *Tags* screen in the sidebar lists every tag with how many cards and notes have it. *Rename* changes a tag everywhere at once; renaming onto a name another tag already has is refused, so two tags are never merged by accident. *Find* opens that tag in *Search*.

**Reviewing a tag across decks.**

- A *Review by tag* section on the Study Desk lists tags that have due cards, and reviews those cards from every active deck in one go. Each card shows which deck it is from.
- Ratings are saved and scheduled exactly as in a normal review. Your decks' own review sessions are left alone.

**Tags in and out.**

- *Decks from Anki* now brings each note's tags onto every card the note adds, including reversed, cloze and picture cards. Tags Synapse can't take (too long, or more than 20 on one card) are counted in the preview.
- A CSV or TSV file of cards can have a third column of tags, separated by spaces, when its header row names it *Tags*. Exporting a deck adds that column when any of its cards has a tag. Files with two columns work exactly as before.

## Backups

**There is one migration (0020), adding the tag tables.** Backups made with 0.10.0 can't be restored into 0.9.1 or older, which say the backup is from a newer version. Backups from 0.9.1 and older still restore into 0.10.0, with no tags. The backup format itself hasn't changed: tags are inside the database the backup already carries.

## Not tried yet

- Everything was checked by automated tests: 361 for the screens, plus 421 for the Rust side and 11 that run the real helper processes. Each part was also tried in the real app on Linux with a scratch database.
- Nothing has been tried on Windows yet.

Windows x64 installers:

- Synapse_0.10.0_x64_en-US.msi
- Synapse_0.10.0_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
