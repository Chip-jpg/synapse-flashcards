import { cleanup } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
  clearMocks();
});

// jsdom has no modal dialogs. This stands in for the parts Synapse relies on,
// behaving as browsers do: `showModal` opens the dialog; `close` closes it with
// an optional return value, hands focus back to what had it before, and then
// fires "close" in a later task; Escape cancels the open dialog.
const previousFocus = new WeakMap<HTMLDialogElement, Element | null>();
const returnValues = new WeakMap<HTMLDialogElement, string>();

Object.defineProperty(HTMLDialogElement.prototype, "returnValue", {
  configurable: true,
  get(this: HTMLDialogElement) {
    return returnValues.get(this) ?? "";
  },
  set(this: HTMLDialogElement, value: string) {
    returnValues.set(this, String(value));
  },
});

HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
  if (this.open) return;
  previousFocus.set(this, document.activeElement);
  this.open = true;
};

HTMLDialogElement.prototype.close = function (this: HTMLDialogElement, returnValue?: string) {
  if (!this.open) return;
  this.open = false;
  if (returnValue !== undefined) this.returnValue = returnValue;
  const previous = previousFocus.get(this);
  if (previous instanceof HTMLElement) previous.focus();
  setTimeout(() => this.dispatchEvent(new Event("close")));
};

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  const dialog = document.querySelector<HTMLDialogElement>("dialog[open]");
  if (dialog && dialog.dispatchEvent(new Event("cancel", { cancelable: true }))) dialog.close();
});
