import { clearMocks, mockConvertFileSrc } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { keptFileUrl, storePicture, type Asset } from "./api";
import { fakeBackend } from "./test/backend";

const HASH = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

afterEach(() => clearMocks());

describe("kept files", () => {
  it("are loaded through Synapse's own scheme, by hash", () => {
    mockConvertFileSrc("linux");
    expect(keptFileUrl(HASH)).toBe(`synapse://localhost/${HASH}`);
    mockConvertFileSrc("windows");
    expect(keptFileUrl(HASH)).toBe(`http://synapse.localhost/${HASH}`);
  });

  it("send a pasted picture's bytes as they are", async () => {
    const kept: Asset = {
      id: 1,
      sha256: HASH,
      mediaType: "image/png",
      size: 3,
      createdAt: "2026-10-20T12:00:00.000Z",
    };
    const backend = fakeBackend({ store_picture: () => kept });
    const bytes = new Uint8Array([0x89, 0x50, 0x4e]);

    await expect(storePicture(bytes)).resolves.toEqual(kept);
    expect(backend.argsOf("store_picture")).toEqual([bytes]);
  });
});
