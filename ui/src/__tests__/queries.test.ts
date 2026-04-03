import { describe, it, expect } from "vitest";
import { qk } from "@/lib/queries";

describe("query keys", () => {
  it("agents key is stable", () => {
    expect(qk.agents).toEqual(["agents"]);
  });

  it("agent key includes name", () => {
    expect(qk.agent("Arty")).toEqual(["agents", "Arty"]);
  });

  it("agentChannels key includes agent name", () => {
    expect(qk.agentChannels("Arty")).toEqual(["agents", "Arty", "channels"]);
  });

  it("tools key is stable", () => {
    expect(qk.tools).toEqual(["tools"]);
  });

  it("yamlTools key is stable", () => {
    expect(qk.yamlTools).toEqual(["yaml-tools"]);
  });

  it("mcpServers key is stable", () => {
    expect(qk.mcpServers).toEqual(["mcp"]);
  });

  it("secrets key is stable", () => {
    expect(qk.secrets).toEqual(["secrets"]);
  });

  it("skills key is stable", () => {
    expect(qk.skills).toEqual(["skills"]);
  });

  it("channels key is stable", () => {
    expect(qk.channels).toEqual(["channels"]);
  });

  it("activeChannels key is stable", () => {
    expect(qk.activeChannels).toEqual(["channels", "active"]);
  });

  it("cron key is stable", () => {
    expect(qk.cron).toEqual(["cron"]);
  });

  it("cronRuns key includes job id", () => {
    expect(qk.cronRuns("job-123")).toEqual(["cron", "job-123", "runs"]);
  });

  it("cronRunsAll key is stable", () => {
    expect(qk.cronRunsAll).toEqual(["cron", "runs"]);
  });

  it("memoryStats key is stable", () => {
    expect(qk.memoryStats).toEqual(["memory", "stats"]);
  });

  it("audit key includes params", () => {
    const params = { agent: "Arty", limit: "10" };
    expect(qk.audit(params)).toEqual(["audit", params]);
  });

  it("config key is stable", () => {
    expect(qk.config).toEqual(["config"]);
  });

  it("access key is stable", () => {
    expect(qk.access).toEqual(["access"]);
  });

  it("usage key includes days", () => {
    expect(qk.usage(7)).toEqual(["usage", 7]);
  });

  it("dailyUsage key includes days", () => {
    expect(qk.dailyUsage(30)).toEqual(["usage", "daily", 30]);
  });

  it("providerModels key includes provider", () => {
    expect(qk.providerModels("openai")).toEqual(["providers", "openai", "models"]);
  });

  it("webhooks key is stable", () => {
    expect(qk.webhooks).toEqual(["webhooks"]);
  });

  it("approvals key is stable", () => {
    expect(qk.approvals).toEqual(["approvals"]);
  });

  it("backups key is stable", () => {
    expect(qk.backups).toEqual(["backups"]);
  });

  it("sessions key includes agent", () => {
    expect(qk.sessions("Arty")).toEqual(["sessions", "list", "Arty"]);
  });

  it("sessionMessages key includes session id", () => {
    expect(qk.sessionMessages("s-1")).toEqual(["sessions", "s-1", "messages"]);
  });

  it("providers key is stable", () => {
    expect(qk.providers).toEqual(["providers"]);
  });

  it("providerTypes key is stable", () => {
    expect(qk.providerTypes).toEqual(["provider-types"]);
  });

  it("providerActive key is stable", () => {
    expect(qk.providerActive).toEqual(["provider-active"]);
  });

  it("mediaDrivers key is stable", () => {
    expect(qk.mediaDrivers).toEqual(["media-drivers"]);
  });

  it("oauthAccounts key is stable", () => {
    expect(qk.oauthAccounts).toEqual(["oauth", "accounts"]);
  });

  it("oauthBindings key includes agent", () => {
    expect(qk.oauthBindings("Arty")).toEqual(["oauth", "bindings", "Arty"]);
  });
});

describe("query key referential stability", () => {
  it("static keys are the same reference across accesses", () => {
    expect(qk.agents).toBe(qk.agents);
    expect(qk.secrets).toBe(qk.secrets);
    expect(qk.tools).toBe(qk.tools);
  });

  it("dynamic keys produce equal (but not same) arrays", () => {
    const a = qk.agent("X");
    const b = qk.agent("X");
    expect(a).toEqual(b);
    expect(a).not.toBe(b); // new array each call
  });
});
