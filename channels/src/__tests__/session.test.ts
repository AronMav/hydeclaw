import { describe, test, expect } from "bun:test";
import { calcBackoff } from "../session";
import { wsToHttp } from "../config";

describe("calcBackoff", () => {
  test("no failures uses base interval", () => {
    expect(calcBackoff(0, 5)).toBe(5000);
  });

  test("first failure uses base interval", () => {
    expect(calcBackoff(1, 5)).toBe(5000);
  });

  test("second failure doubles", () => {
    expect(calcBackoff(2, 5)).toBe(10000);
  });

  test("third failure quadruples", () => {
    expect(calcBackoff(3, 5)).toBe(20000);
  });

  test("caps at 300s", () => {
    expect(calcBackoff(100, 5)).toBe(300000);
  });

  test("custom base interval", () => {
    expect(calcBackoff(1, 10)).toBe(10000);
    expect(calcBackoff(2, 10)).toBe(20000);
  });
});

describe("wsToHttp (imported from config)", () => {
  test("converts ws to http", () => {
    expect(wsToHttp("ws://localhost:18789")).toBe("http://localhost:18789");
  });

  test("converts wss to https", () => {
    expect(wsToHttp("wss://example.com/path")).toBe("https://example.com/path");
  });
});
