import { describe, it, expect } from "vitest";
import { add } from "./lib/math";
describe("add", () => { it("works", () => expect(add(1, 1)).toBe(2)); });
