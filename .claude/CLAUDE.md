# Synapse — project memory

The full project breakdown (product scope, architecture, data-model invariants, validation commands,
release process, roadmap) lives in `PROJECT_BREAKDOWN.md` next to this file and is loaded below.
Use it whenever project specs are needed.

It is a snapshot as of v0.5.0 (released from `390fa47`). When it disagrees with the code or README, the
code and README win; point out the drift rather than trusting the snapshot.

@PROJECT_BREAKDOWN.md

## Safety rules when running the app

These come from two past incidents: a process-tree shutdown that targeted protected system processes
because PIDs had been recycled, and a task that checked whether the real database existed despite a
no-touch rule.

- Only stop processes this task started. Record their PIDs at launch, confirm each PID still belongs
  to that process (name and start time) before stopping it, and never use broad kill patterns.
- Never touch the real Synapse database or its OS data directory, not even to check it exists or to
  prove it wasn't accessed. For experiments, always run a debug build with `SYNAPSE_DB_DIR` pointed
  at a scratch folder (see README). Without it, even a dev build uses the real database, and release
  builds ignore the override entirely.
