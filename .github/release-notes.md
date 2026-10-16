Unsaved-work protection, plus the first automated tests for the interface. Existing study behavior, FSRS scheduling, and data lifecycle rules are unchanged.

- Leaving a form with unsaved text now asks first. If you've typed into a deck, card, or note form and then choose a place in the sidebar or close the window, Synapse asks whether to discard it. *Keep editing* (or Escape) takes you back to the form with your text intact. A form you haven't changed, and *Cancel*, work as before.
- The interface now has automated tests (`npm test`). They cover reviewing and its keyboard shortcuts, confirmations, restoring and out-of-date actions, the backup flow, and the unsaved-changes prompt.

Windows x64 installers:

- Synapse_0.3.2_x64_en-US.msi
- Synapse_0.3.2_x64-setup.exe

The installers are unsigned, so Windows SmartScreen may warn on first run. They were built on GitHub's Windows machines by the new Release workflow, after its tests passed.
