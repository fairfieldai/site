import { describe, expect, it } from "vitest";

import { joinPath, safeReturnPath } from "./auth";

describe("safeReturnPath", () => {
  it.each(["/", "/events/#event-1", "/account/"])("keeps the site path %j", (path) => {
    expect(safeReturnPath(path)).toBe(path);
  });

  it.each([null, undefined, 42, "", "events/", "//evil.example/", "https://evil.example/"])(
    "replaces %j with the home page",
    (path) => {
      expect(safeReturnPath(path)).toBe("/");
    },
  );
});

describe("joinPath", () => {
  it("links to /join/ alone for the home page", () => {
    expect(joinPath()).toBe("/join/");
    expect(joinPath("/")).toBe("/join/");
  });

  it("encodes the return path, including its fragment", () => {
    expect(joinPath("/events/#event-1")).toBe("/join/?next=%2Fevents%2F%23event-1");
  });

  it("drops return paths to other sites", () => {
    expect(joinPath("//evil.example/")).toBe("/join/");
  });
});
