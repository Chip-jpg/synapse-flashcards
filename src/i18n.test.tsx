import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import App from "./App";
import { fakeBackend } from "./test/backend";
import { en, it as italian, I18nProvider, useLang, useT } from "./i18n";

function Example() {
  const { lang, setLang } = useLang();
  const { t, plural } = useT();
  return <><span>{lang}</span><p>{t("common.greeting", { name: "Ada" })}</p><p>{plural(2, "common.cardOne", "common.cardOther")}</p><button onClick={() => setLang("it")}>Italiano</button></>;
}

describe("language", () => {
  it("uses the browser language on first run", () => {
    localStorage.removeItem("synapse.lang");
    const language = vi.spyOn(navigator, "language", "get").mockReturnValue("it-IT");
    render(<I18nProvider><Example /></I18nProvider>);
    expect(screen.getByText("Ciao, Ada")).toBeTruthy();
    expect(document.documentElement.lang).toBe("it");
    language.mockRestore();
  });
  it("persists changes and interpolates plurals", () => {
    const view = render(<I18nProvider><Example /></I18nProvider>);
    fireEvent.click(screen.getByRole("button", { name: "Italiano" }));
    expect(screen.getByText("2 carte")).toBeTruthy();
    expect(localStorage.getItem("synapse.lang")).toBe("it");
    view.unmount();
    render(<I18nProvider><Example /></I18nProvider>);
    expect(screen.getByText("Ciao, Ada")).toBeTruthy();
  });
  it("switches the app navigation and desk to Italian", async () => {
    const user = userEvent.setup();
    fakeBackend({});
    render(<I18nProvider><App /></I18nProvider>);
    await screen.findByRole("button", { name: "Create deck" });
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Language" }), "it");
    expect(screen.getByRole("button", { name: "Impostazioni" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Scrivania" }));
    expect(await screen.findByRole("heading", { name: "Scrivania" })).toBeTruthy();
  });
  it("has an Italian entry for every English entry", () => {
    expect(Object.keys(italian).sort()).toEqual(Object.keys(en).sort());
  });
});
