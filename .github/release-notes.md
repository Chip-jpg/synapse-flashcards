This release finishes the **Italian** interface. The screens that stayed in English in 0.11.0 — notes, sources, quizzes, the tutor, tags and a handful of helpers — are now translated. **There are no new migrations.** Backups from 0.11.0 and 0.11.1 open in either version.

## What's new

**Italiano / English, now complete.**

- Notes (including the Markdown note view).
- Sources, source search, source pages and scanned pages.
- Quizzes and card suggestions.
- The AI tutor.
- Tags, card help, *Read with*, picture helpers, and the generic error message.

A few things stay in English on purpose: Rust backend errors, the model's own output, file and product names, and the prompts sent to the model. Some screens still print an error directly, so "Something went wrong." can appear in English there. The *Language / Lingua* setting is unchanged: Synapse follows your system language on first run and remembers your choice.

## Backups

No new migrations. Backups made with 0.11.0 and 0.11.1 restore into each other. The two migrations from 0.11.0 still apply: backups made with 0.11.0 or 0.11.1 can't be restored into 0.10.0 or older.

## Pull requests in this release

#83, #84, #85, #86, #87.

## Not tried yet

- Everything was checked by automated tests: the screen suite plus the full Rust suite, on GitHub's Windows machines for every pull request.
- The Italian text was reviewed as written but hasn't been tried in the real app on Windows yet. Please report anything that reads oddly.

Windows x64 installers:

- Synapse_0.11.1_x64_en-US.msi
- Synapse_0.11.1_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
