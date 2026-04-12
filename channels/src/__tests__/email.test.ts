import { describe, it, expect, vi } from "vitest";
import { createEmailDriver } from "../drivers/email";

describe("Email Driver", () => {
  it("initializes correctly with config", () => {
    const bridge = {
      checkAccess: vi.fn(),
      createPairingCode: vi.fn(),
      sendMessage: vi.fn().mockReturnValue({ result: Promise.resolve("ok") }),
    } as any;

    const driver = createEmailDriver(
      bridge,
      "test-pass",
      { imap_host: "imap.test.com", imap_user: "bot@test.com" },
      "en",
      "typing"
    );

    expect(driver.start).toBeDefined();
    expect(driver.stop).toBeDefined();
    expect(driver.onAction).toBeDefined();
  });

  // mailparser and imapflow are complex to unit test without heavy mocking,
  // but we can verify the driver structure.
});
