import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import { fakeBackend } from "./test/backend";

function open() {
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  return screen.getByRole("dialog", { name: "Command palette" });
}

describe("command palette", () => {
  it("opens with Ctrl+K and filters without accents", async () => {
    fakeBackend({
      get_decks: () => [
        {
          id: 1,
          name: "Caffè",
          description: null,
          isSample: false,
          dueCount: 0,
        },
      ],
    });
    const user = userEvent.setup();
    render(<App />);
    const dialog = open();
    const input = within(dialog).getByRole("combobox");
    await screen.findByRole("option", { name: "Open deck: Caffè" });
    await user.type(input, "caffe");
    expect(within(dialog).getAllByRole("option")).toHaveLength(1);
    expect(
      within(dialog)
        .getByRole("option", { name: "Open deck: Caffè" })
        .getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("navigates to Settings with Enter", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    const dialog = open();
    await user.type(
      within(dialog).getByRole("combobox"),
      "settings{Enter}",
    );
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
    expect(await screen.findByRole("heading", { name: "Settings" })).toBeTruthy();
  });

  it("closes with Escape and restores focus", async () => {
    fakeBackend();
    render(<App />);
    const button = screen.getByRole("button", { name: "Settings" });
    button.focus();
    const dialog = open();
    fireEvent.keyDown(within(dialog).getByRole("combobox"), {
      key: "Escape",
    });
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
    expect(document.activeElement).toBe(button);
  });

  it("opens a deck", async () => {
    fakeBackend();
    const user = userEvent.setup();
    render(<App />);
    const dialog = open();
    await screen.findByRole("option", { name: "Open deck: Biology" });
    await user.type(
      within(dialog).getByRole("combobox"),
      "biology{Enter}",
    );
    expect(await screen.findByRole("button", { name: "Add card" })).toBeTruthy();
  });
});
