import { describe, expect, it } from "vitest";
import { type PageProgress, recognisePicture, recogniseSourcePages, stopRecognition } from "./api";
import { fakeBackend, sendOnChannel } from "./test/backend";

describe("text recognition calls", () => {
  it("hears each page of a source as it finishes, in order", async () => {
    const backend = fakeBackend({
      recognise_source_pages: (args) => {
        // Sent out of order: the channel still delivers them in order.
        sendOnChannel(args.onProgress, 1, { page: 4, done: 2, total: 2, problem: "noPicture" });
        sendOnChannel(args.onProgress, 0, { page: 2, done: 1, total: 2, problem: null });
        return { read: [2], skipped: [{ page: 4, problem: "noPicture" }], stopped: false };
      },
    });
    const heard: PageProgress[] = [];
    const result = await recogniseSourcePages(7, null, (progress) => heard.push(progress));
    expect(result).toEqual({
      read: [2],
      skipped: [{ page: 4, problem: "noPicture" }],
      stopped: false,
    });
    expect(heard.map((progress) => progress.page)).toEqual([2, 4]);
    expect(backend.argsOf("recognise_source_pages")[0]).toMatchObject({ sourceId: 7, pages: null });
  });

  it("reads a picture and can be stopped", async () => {
    const backend = fakeBackend();
    const reading = await recognisePicture(3);
    expect(reading.status).toBe("read");
    if (reading.status === "read") {
      expect(reading.recognition.assetId).toBe(3);
      expect(reading.recognition.lines[0].words).toHaveLength(3);
    }
    await stopRecognition();
    expect(backend.calls).toContain("stop_recognition");
  });
});
