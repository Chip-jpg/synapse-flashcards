Fit FSRS to your own reviews, see your study statistics, work on several cards at once, move cards in and out of spreadsheets, and get help with cards you keep forgetting. Existing schedules, review history, data lifecycle rules, suggested cards, file import, and backups made by earlier versions are unchanged.

- Settings: a new *Settings* screen can fit FSRS's parameters to your own review history once there are at least 400 reviews to learn from. It shows how well the parameters in use and the fitted ones predict your reviews, and how a new card's intervals would change. Nothing changes until you choose *Apply*, and then only ratings made afterwards use them; cards already scheduled keep their due dates. *Reset to defaults* goes back. One set is used for every deck, and fitting can be stopped at any time.
- Statistics: a new *Statistics* screen shows reviews per day over the last 30 days, true retention (the share of reviews of graduated cards not rated *Again*), your cards by state, and how many cards fall due on each of the next 14 days, for all decks or one. They're counts, not scores: no streaks or mastery percentages.
- Decks: *Search cards* shows only a deck's cards whose front or back contains what you type. Tick cards one by one, or *Select all*, then *Delete selected* or *Move selected* to another deck, after asking. Moved cards keep their schedule and review history; the Sample deck and archived decks are refused.
- Backup & restore: *Cards in a spreadsheet* adds front/back cards to a deck from a CSV or TSV file, after showing every row and naming the ones that break a card's rules or repeat a front already in the deck. Everything is added together, up to 5,000 cards per file. *Export as CSV* saves a deck's cards as a spreadsheet; it holds only their text, so it isn't a backup.
- Reviewing: after you answer *Again* on a card three times in a row, Synapse offers help with it once the answer is shown: a model in Ollama, on your computer, writes a mnemonic, a simpler explanation, an analogy, or the question asked another way, from the card's question and answer only. Any card can ask for help after reveal. It's a draft; nothing is saved unless you choose *Add as a note*, and rating never waits for it. Without Ollama, reviewing is unchanged.

Known issue: an exported CSV writes a card's text as it is, so a side that starts with `=`, `+`, `-` or `@` may be read as a formula when the file is opened in a spreadsheet app. Take care opening an export whose cards came from a source you don't trust.

This version adds one table to the database, for fitted parameters. A backup made by 0.7.0 can't be restored into an earlier version; backups from earlier versions restore as before.

Windows x64 installers:

- Synapse_0.7.0_x64_en-US.msi
- Synapse_0.7.0_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
