Suggested cards: a model running on your own computer can now suggest cards from a note. It's optional, works only with [Ollama](https://ollama.com) installed on the same computer, and changes nothing else. Existing study behavior, FSRS scheduling, data lifecycle rules, and the backup format are unchanged.

- Open a note and choose *Suggest cards*. Pick one of your Ollama models, and it suggests up to ten cards from the note. *Stop* ends a suggestion that's taking too long.
- Every suggested card shows the passage of your note it comes from. Cards that can't be traced back to your note are left out, and you're told how many. If an answer uses words your note doesn't, they're listed so you can check them.
- Nothing is saved until you've checked the cards. Edit or remove any, choose a deck, and save them all at once; they're checked like cards you type, and are due straight away. Leaving or closing first asks whether to discard them.
- Your note never leaves your computer. Synapse asks only Ollama at its usual local address, never Ollama's cloud models, and only when you press *Suggest cards*.
- Notes of up to 12,000 characters can be used for suggestions. How good the cards are depends on the model: larger ones (3B and up) do noticeably better than very small ones.

Windows x64 installers:

- Synapse_0.4.0_x64_en-US.msi
- Synapse_0.4.0_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the Release workflow, after its tests passed.
