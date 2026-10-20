A speed release: reviewing, the Study Desk, deck screens and Settings stay fast as your review history grows. Nothing you see or do changes, your cards are scheduled exactly as before, and nothing is removed or rewritten.

Every rating Synapse records is kept, so after a few years of study the review history holds hundreds of thousands of rows. Until now several everyday questions read that whole history each time they were asked: which cards were already reviewed today (to keep cloze siblings apart and to show today's count), how many times in a row a card was answered *Again* (to offer help), and which card comes next. 0.7.3 answers the same questions while reading only what each one needs.

Measured on a large test collection (36,000 cards and 300,000 ratings), on the same questions before and after:

| What happens | 0.7.2 | 0.7.3 |
| :-- | --: | --: |
| Showing the next card in a review, after each rating | 49 ms | 1.7 ms |
| Updating a session's "Card 2 of 5" count | 48 ms | 1.7 ms |
| Due counts for 20 decks on the Study Desk | 69 ms | 20 ms |
| Today's reviews on the Study Desk | 25 ms | 0.2 ms |
| The Statistics screen's last 30 days | 48 ms | 28 ms |
| Next card in one deck of 52,000 cards | 18 ms | 0.5 ms |

With a small collection the difference is too short to notice; it grows with your history.

- Reviewing: the review history now has two lookup indexes, one by card and one by time (migration 0015), so finding today's ratings or one card's ratings no longer reads every rating ever made. Picking the next card looks for the earliest due card and the oldest new card directly, instead of sorting every due card of the deck after each rating. Cards come in the same order as before: due cards earliest first, then new cards oldest first.
- Decks: adding cards, renaming, archiving, moving cards and choosing a deck for a spreadsheet import check the deck without loading all of its cards first. They refuse the same things they refused before.
- Settings: counting how many of your reviews can be used to fit FSRS no longer copies your whole rating history first.
- Notes and decks: a note shown formatted is read once instead of twice, and a deck screen with many cards keeps its search results and finds cloze siblings without going through the whole deck for every card.

Backups: this release adds migration 0015 (the two indexes; no table, card or rule changes). A backup made with 0.7.3 can't be restored into 0.7.2 or earlier, which will say it comes from a newer version. Backups from earlier versions restore into 0.7.3 as before, and get the indexes when they're opened.

Windows x64 installers:

- Synapse_0.7.3_x64_en-US.msi
- Synapse_0.7.3_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
