import { act, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ExportOutcome, ImportCheck } from "./api";
import App from "./App";
import { CREATED, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const BACKUP: ImportCheck = {
  status: "ready",
  token: 42,
  fileName: "synapse-export-2026-10-01.zip",
  exportedAt: CREATED,
};

function sidebar() {
  return screen.getByRole("navigation", { name: "Synapse", hidden: true });
}

/** Whether the sidebar can be used. While it's inert, nothing in it can be pressed or focused. */
function sidebarLocked() {
  return sidebar().hasAttribute("inert");
}

/** Starts Synapse and opens Backup & restore from the sidebar. */
async function openBackup(user: UserEvent, handlers: Record<string, Handler>) {
  const backend = fakeBackend(handlers);
  render(<App />);
  await screen.findByRole("button", { name: "Create deck" });
  await user.click(within(sidebar()).getByRole("button", { name: "Backup & restore" }));
  await screen.findByRole("heading", { name: "Backup & restore" });
  return backend;
}

describe("restoring a backup", () => {
  it("locks the sidebar while the backup is checked and waits for an answer", async () => {
    const user = userEvent.setup();
    const checking = deferred<ImportCheck>();
    const backup = await openBackup(user, {
      prepare_import: () => checking.promise,
      cancel_import: () => null,
    });
    expect(sidebarLocked()).toBe(false);

    await user.click(screen.getByRole("button", { name: "Import data" }));

    expect(screen.getByText("Checking the backup…")).toBeTruthy();
    expect(sidebarLocked()).toBe(true);

    await act(async () => checking.resolve(BACKUP));

    const question = await screen.findByText(
      /^Replace all your study data with synapse-export-2026-10-01\.zip/,
    );
    expect(document.activeElement).toBe(question);
    expect(sidebarLocked()).toBe(true);

    await user.click(screen.getByRole("button", { name: "Keep current data" }));

    expect(sidebarLocked()).toBe(false);
    expect(backup.argsOf("cancel_import")).toEqual([{ token: 42 }]);
    expect(backup.argsOf("confirm_import")).toEqual([]);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Import data" }));
  });

  it("keeps the sidebar locked while restoring, then reloads the Study Desk", async () => {
    const user = userEvent.setup();
    const restoring = deferred();
    const backup = await openBackup(user, {
      prepare_import: () => BACKUP,
      confirm_import: () => restoring.promise,
    });
    await user.click(screen.getByRole("button", { name: "Import data" }));

    await user.click(await screen.findByRole("button", { name: "Replace my data" }));

    expect(screen.getByText("Restoring your backup…")).toBeTruthy();
    expect(sidebarLocked()).toBe(true);
    // A second press while restoring sends nothing more.
    await user.click(screen.getByRole("button", { name: "Replace my data" }));
    expect(backup.argsOf("confirm_import")).toEqual([{ token: 42 }]);

    await act(async () => restoring.resolve(null));

    expect(
      await screen.findByText(
        "Backup restored from synapse-export-2026-10-01.zip. Everything shown now comes from that backup.",
      ),
    ).toBeTruthy();
    await vi.waitFor(() => expect(sidebarLocked()).toBe(false));
    // The Study Desk loaded once at launch, and again from the restored data.
    expect(backup.argsOf("get_decks")).toHaveLength(2);
  });

  it("says why a restore failed, and unlocks the sidebar", async () => {
    const user = userEvent.setup();
    await openBackup(user, {
      prepare_import: () => BACKUP,
      confirm_import: () =>
        refuse("failed", "The backup couldn't be restored. Your data hasn't changed."),
    });
    await user.click(screen.getByRole("button", { name: "Import data" }));

    await user.click(await screen.findByRole("button", { name: "Replace my data" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "The backup couldn't be restored. Your data hasn't changed.",
    );
    await vi.waitFor(() => expect(sidebarLocked()).toBe(false));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Import data" }));
  });

  it("changes nothing when the file window is closed without choosing", async () => {
    const user = userEvent.setup();
    const backup = await openBackup(user, { prepare_import: () => ({ status: "cancelled" }) });

    await user.click(screen.getByRole("button", { name: "Import data" }));

    // The sidebar unlocks a render or two after the check ends, as each screen
    // passes the change up.
    await vi.waitFor(() => expect(sidebarLocked()).toBe(false));
    expect(screen.queryByText("Checking the backup…")).toBeNull();
    expect(backup.argsOf("prepare_import")).toHaveLength(1);
    expect(screen.queryByText(/^Replace all your study data/)).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("exporting a backup", () => {
  it("locks the sidebar while the export is saved, then says where it went", async () => {
    const user = userEvent.setup();
    const exporting = deferred<ExportOutcome>();
    const backup = await openBackup(user, { export_data: () => exporting.promise });

    await user.click(screen.getByRole("button", { name: "Export data" }));

    expect(screen.getByText("Preparing your export…")).toBeTruthy();
    expect(sidebarLocked()).toBe(true);
    // Import waits for the export to finish.
    await user.click(screen.getByRole("button", { name: "Import data" }));
    expect(backup.argsOf("prepare_import")).toEqual([]);

    await act(async () => exporting.resolve({ status: "saved", fileName: "synapse-export.zip" }));

    expect(await screen.findByText("Export saved as synapse-export.zip.")).toBeTruthy();
    await vi.waitFor(() => expect(sidebarLocked()).toBe(false));
    expect(backup.argsOf("export_data")).toHaveLength(1);
  });
});
