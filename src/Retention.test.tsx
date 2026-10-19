import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { RetentionPreview, RetentionSettings } from "./api";
import App from "./App";
import { formatRemembered, formatReviews } from "./Retention";
import { deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const SETTINGS: RetentionSettings = {
  version: 2,
  global: 90,
  default: 90,
  minimum: 70,
  maximum: 97,
  decks: [
    { id: 1, name: "Biology", percent: null },
    { id: 2, name: "History", percent: 85 },
  ],
  archivedOwn: 0,
};

const PREVIEW: RetentionPreview = {
  version: 2,
  cards: 141,
  days: 90,
  current: { reviewsPerDay: 3.5027, remembered: 132.14 },
  proposed: { reviewsPerDay: 2.7694, remembered: 131.09 },
  ownRatings: true,
};

function sidebar() {
  return screen.getByRole("navigation", { name: "Synapse", hidden: true });
}

/** Starts Synapse, opens Settings, and returns the desired retention panel. */
async function openRetention(user: UserEvent, handlers: Record<string, Handler>) {
  const backend = fakeBackend(handlers);
  render(<App />);
  await screen.findByRole("button", { name: "Create deck" });
  await user.click(within(sidebar()).getByRole("button", { name: "Settings" }));
  await screen.findByRole("heading", { name: "Settings" });
  const panel = screen.getByRole("region", { name: "Desired retention" });
  await within(panel).findByRole("table", { name: "Aims for" });
  return { backend, panel };
}

function rowValues(table: HTMLElement, name: RegExp) {
  const row = within(table).getByRole("row", { name });
  return within(row)
    .getAllByRole("cell")
    .map((cell) => cell.textContent);
}

describe("desired retention", () => {
  it("shows the value for all decks and each deck's own", async () => {
    const user = userEvent.setup();
    const { panel } = await openRetention(user, {
      get_retention: () => ({ ...SETTINGS, archivedOwn: 2 }),
    });

    const table = within(panel).getByRole("table", { name: "Aims for" });
    expect(rowValues(table, /^All decks/)[0]).toBe("90%");
    expect(rowValues(table, /^Biology/)[0]).toBe("90% (all decks)");
    expect(rowValues(table, /^History/)[0]).toBe("85%");
    expect(within(panel).getByText(/sample deck and archived decks follow/)).toBeTruthy();
    expect(within(panel).getByText(/2 archived decks keep a value of their own/)).toBeTruthy();
  });

  it("estimates the reviews before applying a value for all decks", async () => {
    const user = userEvent.setup();
    const estimating = deferred<RetentionPreview>();
    const { backend, panel } = await openRetention(user, {
      get_retention: () => SETTINGS,
      preview_retention: () => estimating.promise,
      set_retention: () => ({ ...SETTINGS, version: 3, global: 80 }),
    });

    await user.click(within(panel).getByRole("button", { name: "Change for all decks" }));
    const choice = within(panel).getByLabelText("Desired retention for all decks");
    await waitFor(() => expect(document.activeElement).toBe(choice));
    // The value in use can't be estimated against itself.
    const estimate = within(panel).getByRole("button", { name: "Estimate reviews" });
    expect(estimate.getAttribute("aria-disabled")).toBe("true");
    await user.click(estimate);
    expect(backend.calls).not.toContain("preview_retention");
    expect(within(panel).getByRole("option", { name: "90% (FSRS's recommendation)" })).toBeTruthy();
    expect(within(panel).getAllByRole("option")).toHaveLength(28);

    await user.selectOptions(choice, "80");
    await user.click(estimate);
    expect(within(panel).getByText("Estimating your reviews…")).toBeTruthy();
    await user.click(estimate);
    expect(backend.argsOf("preview_retention")).toEqual([
      { version: 2, deckId: null, percent: 80 },
    ]);

    await act(async () => estimating.resolve(PREVIEW));
    const heading = await within(panel).findByRole("heading", { name: "Estimated reviews" });
    await waitFor(() => expect(document.activeElement).toBe(heading));
    const table = within(panel).getByRole("table", { name: "Over the next 90 days" });
    expect(within(table).getByRole("columnheader", { name: "In use (90%)" })).toBeTruthy();
    expect(within(table).getByRole("columnheader", { name: "Chosen (80%)" })).toBeTruthy();
    expect(rowValues(table, /^Reviews a day/)).toEqual(["3.5", "2.8"]);
    expect(rowValues(table, /^Cards you'd remember on day 90/)).toEqual([
      "132 of 141 (93.7%)",
      "131 of 141 (93.0%)",
    ]);
    expect(within(panel).getByText(/^An estimate, not a promise/).textContent).toMatch(
      /simulated your 141 cards .* how often you choose each rating/
    );
    expect(backend.calls).not.toContain("set_retention");

    await user.click(within(panel).getByRole("button", { name: "Apply" }));
    expect(backend.argsOf("set_retention")).toEqual([{ version: 2, deckId: null, percent: 80 }]);
    const notice = await within(panel).findByText(/^Decks without a value of their own now aim for 80%/);
    await waitFor(() => expect(document.activeElement).toBe(notice));
    const after = within(panel).getByRole("table", { name: "Aims for" });
    expect(rowValues(after, /^All decks/)[0]).toBe("80%");
  });

  it("lets a deck follow the value for all decks again", async () => {
    const user = userEvent.setup();
    const { backend, panel } = await openRetention(user, {
      get_retention: () => SETTINGS,
      preview_retention: () => ({ ...PREVIEW, ownRatings: false }),
      set_retention: () => ({
        ...SETTINGS,
        version: 3,
        decks: SETTINGS.decks.map((deck) => ({ ...deck, percent: null })),
      }),
    });

    await user.click(within(panel).getByRole("button", { name: "Change History" }));
    const choice = within(panel).getByLabelText("Desired retention for History");
    expect((choice as HTMLSelectElement).value).toBe("85");
    await user.selectOptions(choice, "");
    await user.click(within(panel).getByRole("button", { name: "Estimate reviews" }));

    expect(await within(panel).findByText(/FSRS's average mix of ratings/)).toBeTruthy();
    expect(
      within(panel).getByRole("columnheader", { name: "Chosen (90%, all decks)" })
    ).toBeTruthy();
    expect(
      within(panel).getByText("Have History follow the value for all decks (90%)?")
    ).toBeTruthy();
    await user.click(within(panel).getByRole("button", { name: "Apply" }));

    expect(backend.argsOf("preview_retention")).toEqual([
      { version: 2, deckId: 2, percent: null },
    ]);
    expect(backend.argsOf("set_retention")).toEqual([{ version: 2, deckId: 2, percent: null }]);
    expect(await within(panel).findByText(/^History now follows the value for all decks/)).toBeTruthy();
  });

  it("drops an estimate when the choice changes, and cancels back to its button", async () => {
    const user = userEvent.setup();
    const { backend, panel } = await openRetention(user, {
      get_retention: () => SETTINGS,
      preview_retention: () => PREVIEW,
    });

    await user.click(within(panel).getByRole("button", { name: "Change Biology" }));
    const choice = within(panel).getByLabelText("Desired retention for Biology");
    await user.selectOptions(choice, "95");
    await user.click(within(panel).getByRole("button", { name: "Estimate reviews" }));
    await within(panel).findByRole("heading", { name: "Estimated reviews" });

    await user.selectOptions(choice, "93");
    expect(within(panel).queryByRole("heading", { name: "Estimated reviews" })).toBeNull();
    expect(within(panel).queryByRole("button", { name: "Apply" })).toBeNull();

    await user.click(within(panel).getByRole("button", { name: "Cancel" }));
    expect(within(panel).queryByLabelText("Desired retention for Biology")).toBeNull();
    expect(document.activeElement).toBe(
      within(panel).getByRole("button", { name: "Change Biology" })
    );
    expect(backend.calls).not.toContain("set_retention");
  });

  it("reloads when what's shown is stale, and shows other refusals", async () => {
    const user = userEvent.setup();
    let settings = SETTINGS;
    let previews = 0;
    const { panel } = await openRetention(user, {
      get_retention: () => settings,
      preview_retention: () => {
        previews += 1;
        if (previews === 1) {
          return refuse("failed", "Synapse couldn't estimate your reviews. Nothing was changed.");
        }
        return PREVIEW;
      },
      set_retention: () => {
        settings = { ...SETTINGS, version: 3, global: 75 };
        return refuse(
          "stale",
          "The desired retention changed since this was shown. Synapse has reloaded it."
        );
      },
    });

    await user.click(within(panel).getByRole("button", { name: "Change for all decks" }));
    await user.selectOptions(within(panel).getByLabelText("Desired retention for all decks"), "85");
    await user.click(within(panel).getByRole("button", { name: "Estimate reviews" }));
    expect((await within(panel).findByRole("alert")).textContent).toBe(
      "Synapse couldn't estimate your reviews. Nothing was changed."
    );

    await user.click(within(panel).getByRole("button", { name: "Estimate reviews" }));
    await user.click(await within(panel).findByRole("button", { name: "Apply" }));

    const reloaded = await within(panel).findByText(/^The desired retention changed/);
    await waitFor(() => expect(document.activeElement).toBe(reloaded));
    const table = within(panel).getByRole("table", { name: "Aims for" });
    expect(rowValues(table, /^All decks/)[0]).toBe("75%");
  });

  it("says when there are no cards to estimate with", async () => {
    const user = userEvent.setup();
    const { panel } = await openRetention(user, {
      get_retention: () => SETTINGS,
      preview_retention: () => ({ ...PREVIEW, cards: 0 }),
    });

    await user.click(within(panel).getByRole("button", { name: "Change for all decks" }));
    await user.selectOptions(within(panel).getByLabelText("Desired retention for all decks"), "97");
    await user.click(within(panel).getByRole("button", { name: "Estimate reviews" }));

    expect(await within(panel).findByText(/no cards to study yet/)).toBeTruthy();
    expect(within(panel).queryByRole("table", { name: "Over the next 90 days" })).toBeNull();
    expect(within(panel).getByRole("button", { name: "Apply" })).toBeTruthy();
  });

  it("explains a failure to load, and retries", async () => {
    const user = userEvent.setup();
    let fail = true;
    const backend = fakeBackend({
      get_retention: () => {
        if (fail) {
          fail = false;
          return refuse("failed", "Synapse couldn't load the desired retention.");
        }
        return SETTINGS;
      },
    });
    render(<App />);
    await screen.findByRole("button", { name: "Create deck" });
    await user.click(within(sidebar()).getByRole("button", { name: "Settings" }));
    const panel = await screen.findByRole("region", { name: "Desired retention" });

    expect((await within(panel).findByRole("alert")).textContent).toBe(
      "Synapse couldn't load the desired retention."
    );
    await user.click(within(panel).getByRole("button", { name: "Retry" }));
    expect(await within(panel).findByRole("table", { name: "Aims for" })).toBeTruthy();
    expect(backend.calls.filter((cmd) => cmd === "get_retention")).toHaveLength(2);
  });
});

describe("formatting estimates", () => {
  it("writes reviews and remembered cards in words", () => {
    expect(formatReviews(0.44)).toBe("0.4");
    expect(formatReviews(3.96)).toBe("4.0");
    expect(formatReviews(9.96)).toBe("10");
    expect(formatReviews(1234.4)).toBe("1,234");
    expect(formatRemembered(131.6, 1410)).toBe("132 of 1,410 (9.3%)");
    expect(formatRemembered(0, 0)).toBe("0 of 0 (0.0%)");
  });
});
