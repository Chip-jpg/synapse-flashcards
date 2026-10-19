import { act, render, screen, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { OptimizeOutcome, SchedulingSettings } from "./api";
import App from "./App";
import { formatInterval, formatPercent } from "./Settings";
import { DEFAULTS, deferred, fakeBackend, refuse, type Handler } from "./test/backend";

const FITTED = DEFAULTS.map((value) => value * 2);

const DEFAULT_SETTINGS: SchedulingSettings = {
  version: 0,
  personal: false,
  parameters: DEFAULTS,
  defaults: DEFAULTS,
  reviewsUsed: null,
  appliedAt: null,
  usableReviews: 650,
  minimumReviews: 400,
};

const PERSONAL_SETTINGS: SchedulingSettings = {
  ...DEFAULT_SETTINGS,
  version: 1,
  personal: true,
  parameters: FITTED,
  reviewsUsed: 650,
  appliedAt: "2026-10-19T09:00:00Z",
};

const OPTIMIZED: OptimizeOutcome = {
  status: "optimized",
  token: 7,
  reviewsUsed: 650,
  current: DEFAULTS,
  proposed: FITTED,
  currentError: 0.095,
  proposedError: 0.056,
  better: true,
  intervals: [
    { case: "firstAgain", current: 0.212, proposed: 1.27 },
    { case: "firstHard", current: 1.29, proposed: 7.77 },
    { case: "firstGood", current: 2.31, proposed: 13.86 },
    { case: "firstEasy", current: 8.3, proposed: 49.8 },
    { case: "secondGood", current: 10.96, proposed: 36.06 },
  ],
};

function sidebar() {
  return screen.getByRole("navigation", { name: "Synapse", hidden: true });
}

/** Starts Synapse and opens Settings from the sidebar. */
async function openSettings(user: UserEvent, handlers: Record<string, Handler>) {
  const backend = fakeBackend(handlers);
  render(<App />);
  await screen.findByRole("button", { name: "Create deck" });
  await user.click(within(sidebar()).getByRole("button", { name: "Settings" }));
  await screen.findByRole("heading", { name: "Settings" });
  return backend;
}

describe("personal scheduling", () => {
  it("explains why optimizing isn't possible yet, and doesn't ask Rust", async () => {
    const user = userEvent.setup();
    const backend = await openSettings(user, {
      get_scheduling: () => ({ ...DEFAULT_SETTINGS, usableReviews: 120 }),
    });

    expect(screen.getByText(/FSRS's default parameters/)).toBeTruthy();
    const optimize = screen.getByRole("button", { name: "Optimize" });
    expect(optimize.getAttribute("aria-disabled")).toBe("true");
    expect(optimize.getAttribute("aria-describedby")).toBe("optimize-hint");
    expect(screen.getByText(/^120 of the 400 reviews needed so far/)).toBeTruthy();

    await user.click(optimize);
    expect(backend.calls).not.toContain("optimize_parameters");
    // Nothing to reset while the defaults are in use.
    expect(screen.queryByRole("button", { name: "Reset to defaults" })).toBeNull();
  });

  it("shows the fitted parameters against the ones in use, and applies them", async () => {
    const user = userEvent.setup();
    const optimizing = deferred<OptimizeOutcome>();
    const backend = await openSettings(user, {
      get_scheduling: () => DEFAULT_SETTINGS,
      optimize_parameters: () => optimizing.promise,
      apply_parameters: () => PERSONAL_SETTINGS,
    });

    await user.click(screen.getByRole("button", { name: "Optimize" }));
    expect(screen.getByText("Fitting FSRS to your reviews…")).toBeTruthy();
    // One run at a time.
    await user.click(screen.getByRole("button", { name: "Optimize" }));
    expect(backend.calls.filter((cmd) => cmd === "optimize_parameters")).toHaveLength(1);

    await act(async () => optimizing.resolve(OPTIMIZED));

    const heading = await screen.findByRole("heading", { name: "Fitted to 650 reviews" });
    expect(document.activeElement).toBe(heading);
    expect(screen.getByText(/off by 9\.5% on average .* and 5\.6% with the fitted ones/)).toBeTruthy();
    const table = screen.getByRole("table", { name: "Next review after…" });
    const good = within(table).getByRole("row", { name: /^Good on a new card/ });
    expect(within(good).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "2.3 days",
      "14 days",
    ]);
    const again = within(table).getByRole("row", { name: /^Again on a new card/ });
    expect(within(again).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "5 hours",
      "1.3 days",
    ]);
    // Nothing was saved yet.
    expect(backend.calls).not.toContain("apply_parameters");

    await user.click(screen.getByRole("button", { name: "Apply" }));

    expect(backend.argsOf("apply_parameters")).toEqual([{ token: 7 }]);
    const notice = await screen.findByText(/^Your own parameters are in use/);
    expect(document.activeElement).toBe(notice);
    expect(screen.getByText(/fitted to your own history \(650 reviews\), applied/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Reset to defaults" })).toBeTruthy();
  });

  it("doesn't offer parameters that don't predict better", async () => {
    const user = userEvent.setup();
    await openSettings(user, {
      get_scheduling: () => DEFAULT_SETTINGS,
      optimize_parameters: () => ({ ...OPTIMIZED, better: false }),
    });

    await user.click(screen.getByRole("button", { name: "Optimize" }));

    expect(await screen.findByText(/don't predict your reviews better/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Optimize" }));
  });

  it("stops a run, and stops it when the screen is left", async () => {
    const user = userEvent.setup();
    let finish: (outcome: OptimizeOutcome) => void = () => {};
    const backend = await openSettings(user, {
      get_scheduling: () => DEFAULT_SETTINGS,
      optimize_parameters: () =>
        new Promise<OptimizeOutcome>((resolve) => {
          finish = resolve;
        }),
      stop_optimizing: () => {
        finish({ status: "stopped" });
        return null;
      },
    });

    await user.click(screen.getByRole("button", { name: "Optimize" }));
    await user.click(screen.getByRole("button", { name: "Stop" }));

    expect(await screen.findByText("Optimizing stopped. Nothing was changed.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(backend.calls.filter((cmd) => cmd === "stop_optimizing")).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "Optimize" }));
    await user.click(within(sidebar()).getByRole("button", { name: "Study Desk" }));
    expect(backend.calls.filter((cmd) => cmd === "stop_optimizing")).toHaveLength(2);
  });

  it("shows a refusal from Rust", async () => {
    const user = userEvent.setup();
    await openSettings(user, {
      get_scheduling: () => DEFAULT_SETTINGS,
      optimize_parameters: () =>
        refuse("failed", "Optimizing took too long, so Synapse stopped it. Nothing was changed."),
    });

    await user.click(screen.getByRole("button", { name: "Optimize" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Optimizing took too long, so Synapse stopped it. Nothing was changed."
    );
    expect(screen.getByRole("button", { name: "Optimize" }).getAttribute("aria-disabled")).toBe(
      "false"
    );
  });

  it("resets to the defaults after asking, and reloads when what's shown is stale", async () => {
    const user = userEvent.setup();
    let settings = PERSONAL_SETTINGS;
    let stale = true;
    const backend = await openSettings(user, {
      get_scheduling: () => settings,
      reset_parameters: () => {
        if (stale) {
          stale = false;
          return refuse(
            "stale",
            "The scheduling parameters changed since this was shown. Synapse has reloaded them."
          );
        }
        settings = DEFAULT_SETTINGS;
        return settings;
      },
    });

    await user.click(screen.getByRole("button", { name: "Reset to defaults" }));
    const question = screen.getByText(/^Go back to FSRS's default parameters\?/);
    expect(document.activeElement).toBe(question);
    await user.click(screen.getByRole("button", { name: "Keep my parameters" }));
    expect(backend.calls).not.toContain("reset_parameters");

    await user.click(screen.getByRole("button", { name: "Reset to defaults" }));
    await user.click(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(backend.argsOf("reset_parameters")).toEqual([{ version: 1 }]);
    const reloaded = await screen.findByText(/^The scheduling parameters changed/);
    expect(document.activeElement).toBe(reloaded);

    await user.click(screen.getByRole("button", { name: "Reset to defaults" }));
    await user.click(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(await screen.findByText(/^FSRS's default parameters are in use again/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Reset to defaults" })).toBeNull();
  });
});

describe("formatting", () => {
  it("writes intervals and errors in words", () => {
    expect(formatInterval(0.005)).toBe("7 min");
    expect(formatInterval(0.0001)).toBe("1 min");
    expect(formatInterval(1 / 24)).toBe("1 hour");
    expect(formatInterval(0.212)).toBe("5 hours");
    expect(formatInterval(1)).toBe("1 day");
    expect(formatInterval(2.3065)).toBe("2.3 days");
    expect(formatInterval(36.06)).toBe("36 days");
    expect(formatInterval(1234.4)).toBe("1,234 days");
    expect(formatPercent(0.0951)).toBe("9.5%");
  });
});
