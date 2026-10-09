import { describe, expect, it } from "vitest";
import { fieldChipMenu, toggleNegate } from "./chipMenu";
import { words } from "./queryText";

describe("a chip's menu in the search field", () => {
  it("leads with the field's own entries, then the chip's", () => {
    expect(fieldChipMenu("datalib:group/slack", "Work Slack", false).map((e) => e.id)).toEqual([
      "edit",
      "toggle-negate",
      "copy-name",
      "copy-id",
      "copy-both",
      "open",
      "browse",
    ]);
    const [, negate, firstOfChip] = fieldChipMenu("datalib:group/slack", "Work Slack", true);
    expect(negate.label).toBe("Include Work Slack instead");
    expect(firstOfChip.separator).toBe(true);
  });

  it("excludes with a leading dash, and includes by taking it off", () => {
    const q = "is:document source_id:slack -step:slack/ingest";
    const [, plain, negated] = words(q);
    expect(toggleNegate(plain)).toEqual({ from: 12, to: 12, insert: "-" });
    expect(toggleNegate(negated)).toEqual({ from: 28, to: 29, insert: "" });
  });
});
