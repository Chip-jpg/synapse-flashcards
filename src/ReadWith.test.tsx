import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import type { PageProgress } from "./api";
import {
  Agreement,
  pagesToRead,
  readBy,
  readPicture,
  ReadWith,
  readSourcePages,
  useReader,
  type PictureRead,
} from "./ReadWith";
import { fakeBackend, refuse, sendOnChannel } from "./test/backend";

/** A picture's *Text in picture* action, as a screen would wire `ReadWith`. */
function TextInPicture({ assetId }: { assetId: number }) {
  const state = useReader();
  const [read, setRead] = useState<PictureRead | null>(null);
  const [error, setError] = useState<string | null>(null);
  return (
    <div>
      <ReadWith id="read-with" state={state} />
      <button
        type="button"
        onClick={async () => {
          try {
            setRead(await readPicture(state.reader, assetId));
          } catch (err) {
            setError((err as { message: string }).message);
            void state.reload();
          }
        }}
      >
        Text in picture
      </button>
      {read?.status === "read" && (
        <div>
          <p>
            Recognised by {readBy(read.recognition)}: {read.recognition.text}
          </p>
          <Agreement percent={read.agreement} />
        </div>
      )}
      {error && <p role="alert">{error}</p>}
    </div>
  );
}

const reader = () => screen.getByRole("combobox", { name: "Read with" });

async function optionsLoaded() {
  await waitFor(() =>
    expect(screen.queryByText("Looking for vision models in Ollama…")).toBeNull(),
  );
}

afterEach(() => localStorage.clear());

describe("choosing what reads the text", () => {
  it("offers the built-in reader and Ollama's vision models, built in first", async () => {
    const backend = fakeBackend({
      get_vision_models: () => ["gemma3:4b", "qwen2.5vl:7b"],
    });
    render(<TextInPicture assetId={3} />);
    await optionsLoaded();
    const options = screen.getAllByRole("option").map((option) => option.textContent);
    expect(options).toEqual(["Built in", "gemma3:4b (Ollama)", "qwen2.5vl:7b (Ollama)"]);
    expect((reader() as HTMLSelectElement).value).toBe("");
    expect(screen.getByText(/reads letters without accents/)).toBeTruthy();
    expect(backend.calls).toContain("get_vision_models");
  });

  it("reads with the built-in engine by default, with no agreement shown", async () => {
    const backend = fakeBackend();
    render(<TextInPicture assetId={3} />);
    await optionsLoaded();
    await userEvent.click(screen.getByRole("button", { name: "Text in picture" }));
    expect(
      await screen.findByText("Recognised by the built-in reader: Mitochondria make ATP."),
    ).toBeTruthy();
    expect(backend.argsOf("recognise_picture")).toEqual([{ assetId: 3 }]);
    expect(backend.calls).not.toContain("recognise_picture_with_model");
    expect(screen.queryByText(/of the words the built-in reader found/)).toBeNull();
  });

  it("reads with the chosen model, shows how far it agrees, and remembers the choice", async () => {
    const backend = fakeBackend({
      recognise_picture_with_model: (args) => ({
        status: "read",
        recognition: {
          assetId: args.assetId,
          page: 0,
          engine: "ollama",
          model: args.model,
          text: "La cellula è viva.",
          lines: [],
          width: null,
          height: null,
          createdAt: "2026-10-21T09:05:00Z",
        },
        agreement: 75,
      }),
    });
    const { unmount } = render(<TextInPicture assetId={3} />);
    await optionsLoaded();
    await userEvent.selectOptions(reader(), "qwen2.5vl:7b");
    expect(screen.getByText(/reads accents and handwriting/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Text in picture" }));
    expect(
      await screen.findByText("Recognised by qwen2.5vl:7b in Ollama: La cellula è viva."),
    ).toBeTruthy();
    expect(
      screen.getByText(/has 75% of the words the built-in reader found/).textContent,
    ).toContain("check the text closely");
    expect(backend.argsOf("recognise_picture_with_model")).toEqual([
      { assetId: 3, model: "qwen2.5vl:7b" },
    ]);
    expect(backend.calls).not.toContain("recognise_picture");

    unmount();
    render(<TextInPicture assetId={3} />);
    await waitFor(() => expect((reader() as HTMLSelectElement).value).toBe("qwen2.5vl:7b"));

    // Choosing built in again forgets the model.
    await userEvent.selectOptions(reader(), "");
    expect(localStorage.getItem("synapse.readWith")).toBeNull();
  });

  it("falls back to the built-in reader when the model remembered has left Ollama", async () => {
    localStorage.setItem("synapse.readWith", "llava:7b");
    fakeBackend();
    render(<TextInPicture assetId={3} />);
    await optionsLoaded();
    expect((reader() as HTMLSelectElement).value).toBe("");
  });

  it("goes back to the built-in reader when the chosen model is gone by the time it reads", async () => {
    let models = ["qwen2.5vl:7b"];
    fakeBackend({
      get_vision_models: () => models,
      recognise_picture_with_model: () => {
        models = [];
        return refuse(
          "stale",
          "That model isn't in Ollama any more, or can't read pictures. Choose another.",
        );
      },
    });
    render(<TextInPicture assetId={3} />);
    await optionsLoaded();
    await userEvent.selectOptions(reader(), "qwen2.5vl:7b");
    await userEvent.click(screen.getByRole("button", { name: "Text in picture" }));
    expect((await screen.findByRole("alert")).textContent).toContain("isn't in Ollama any more");
    await waitFor(() => expect((reader() as HTMLSelectElement).value).toBe(""));
    expect(screen.getAllByRole("option")).toHaveLength(1);
  });

  it("explains why only the built-in reader is offered, and checks again on request", async () => {
    let reachable = false;
    const backend = fakeBackend({
      get_vision_models: () =>
        reachable
          ? []
          : refuse(
              "failed",
              "Synapse couldn't reach Ollama on this computer. Check that Ollama is installed and running, then try again.",
            ),
    });
    render(<TextInPicture assetId={3} />);
    expect(
      await screen.findByText(/couldn't reach Ollama.*only the built-in reader is offered/),
    ).toBeTruthy();
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["Built in"]);

    reachable = true;
    await userEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(await screen.findByText(/has no models that can read pictures/)).toBeTruthy();
    expect(backend.calls.filter((call) => call === "get_vision_models")).toHaveLength(2);
  });
});

describe("reading a source's pages with either reader", () => {
  it("sends the model and hears each page as it finishes", async () => {
    const backend = fakeBackend({
      get_pages_to_recognise_with_model: () => [2, 3],
      recognise_source_pages_with_model: (args) => {
        sendOnChannel(args.onProgress, 0, {
          page: 2,
          done: 1,
          total: 2,
          problem: null,
        });
        sendOnChannel(args.onProgress, 1, {
          page: 3,
          done: 2,
          total: 2,
          problem: "unreadablePicture",
        });
        return {
          read: [2],
          skipped: [{ page: 3, problem: "unreadablePicture" }],
          stopped: false,
          agreement: [{ page: 2, percent: 91 }],
        };
      },
    });
    const model = { engine: "ollama", model: "qwen2.5vl:7b" } as const;
    expect(await pagesToRead(model, 7)).toEqual([2, 3]);
    const heard: PageProgress[] = [];
    const result = await readSourcePages(model, 7, null, (progress) => heard.push(progress));
    expect(result.agreement).toEqual([{ page: 2, percent: 91 }]);
    expect(heard.map((progress) => progress.page)).toEqual([2, 3]);
    expect(backend.argsOf("get_pages_to_recognise_with_model")).toEqual([
      { sourceId: 7, model: "qwen2.5vl:7b" },
    ]);
    expect(backend.argsOf("recognise_source_pages_with_model")[0]).toMatchObject({
      sourceId: 7,
      pages: null,
      model: "qwen2.5vl:7b",
    });
  });

  it("uses the built-in engine's commands for the built-in reader", async () => {
    const backend = fakeBackend({
      get_pages_to_recognise: () => [4],
      recognise_source_pages: () => ({
        read: [4],
        skipped: [],
        stopped: false,
      }),
    });
    const builtIn = { engine: "ocrs" } as const;
    expect(await pagesToRead(builtIn, 7)).toEqual([4]);
    expect(await readSourcePages(builtIn, 7, [4], () => {})).toEqual({
      read: [4],
      skipped: [],
      stopped: false,
      agreement: [],
    });
    expect(backend.argsOf("recognise_source_pages")[0]).toMatchObject({
      sourceId: 7,
      pages: [4],
    });
    expect(backend.calls).not.toContain("recognise_source_pages_with_model");
  });
});
