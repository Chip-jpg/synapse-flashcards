Three smaller improvements on top of 0.9.0: pictures now come along when you import decks from Anki, the deleted cards list gets *Select all*, and help with hard cards now works on picture cards too. **There is no migration in this release, so backups stay compatible with 0.9.0 in both directions** (see *Backups* below).

## What's new

**Pictures from Anki front/back notes.**

- Until now, *Decks from Anki* dropped every picture on an ordinary front/back note and kept only its text.
- Now each picture on a note's front or back becomes a picture on that side of the card, in the same order, up to 4 per side. On a reversed card the sides are swapped, so the pictures follow their text.
- The preview before saving says how many pictures will be kept, and how many were left out and why: missing from the package, a kind of file Synapse doesn't show, too large (over 20 MB), or more than 4 on one side. A card whose pictures were left out still keeps its text.
- A side with only a picture and no text is still left out, as before, since every card side needs text.
- Cloze notes still come in without pictures, because cloze cards can't carry pictures yet. Sounds and videos are still dropped and counted.

**Select all among deleted cards.**

- Under *Deleted cards*, once two or more cards can be restored, a *Select all* box ticks them all at once, then *Restore selected* brings them back. It completes *Restore selected* from 0.8.1.
- Cards that can't come back right now (for example a cloze card whose number is no longer in its text) are left out, and the label says how many can be restored.
- Fixed on the way: ticking cards in the active list and in the deleted list could show two selection bars at the same time. Now only one shows, and ticking a card in one list clears what was ticked in the other.

**Help with picture cards.**

- Help with a card (a mnemonic, a simpler explanation, an analogy, or the question asked another way) was not offered on picture cards (image occlusion), because a text-only model can't see the picture.
- Now it is, after three *Again* in a row as on any other card, or by hand after revealing the answer. It is written by a vision model you run yourself in Ollama, such as qwen2.5vl or llama3.2-vision. The model sees the picture with the card's boxes framed in red, plus the card's header and the boxes' labels.
- If no vision model is installed, Synapse says so and doesn't fall back to a text-only model, which would answer without seeing the picture.
- The same rules as the rest of help apply: only Ollama on this computer is asked, nothing is sent until you press the button, nothing is saved unless you choose *Add as a note*, and rating the card never waits for it.
- In a test with qwen2.5vl:3b on a computer without a graphics card, the answers made sense and took about 40 seconds each.

## Backups

**No migration in this release.** Backups made with 0.9.1 restore into 0.9.0, and backups made with 0.9.0 restore into 0.9.1. Backups from 0.8.1 and older still restore into this version.

## Not tried yet

- Everything was checked by automated tests: 319 for the screens, plus 397 for the Rust side and 11 that run the real helper processes. Help with picture cards was also tried against a real Ollama on Linux.
- Nothing has been tried on Windows yet, and the Anki pictures and *Select all* haven't been tried in the real app window.

Windows x64 installers:

- Synapse_0.9.1_x64_en-US.msi
- Synapse_0.9.1_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
