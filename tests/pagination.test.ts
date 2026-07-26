import { describe, expect, it } from "vitest";
import { requestPagination } from "../src/server/pagination.js";

describe("requestPagination", () => {
  it("normalizes invalid values and enforces the configured maximum", () => {
    expect(requestPagination({}, 10, 100)).toEqual({ page: 1, pageSize: 10 });
    expect(requestPagination({ page: "-4", pageSize: "999" }, 10, 100)).toEqual({ page: 1, pageSize: 100 });
    expect(requestPagination({ page: "3.9", pageSize: "12.8" }, 10, 100)).toEqual({ page: 3, pageSize: 12 });
    expect(requestPagination({ page: "nope", pageSize: "nope" }, 10, 100)).toEqual({ page: 1, pageSize: 10 });
  });
});
