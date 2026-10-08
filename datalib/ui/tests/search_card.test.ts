import { describe, expect, it } from "vitest";
import {
  freeText,
  markWords,
  meaningOnly,
  pickedSource,
  setMeaningOnly,
  setSource,
} from "../src/cards/search";

describe("Meaning only, as the query says it", () => {
  /** Only the free text moves; a filter inside the predicate would be searched as words. */
  it("moves only the free text into a qmd_vsearch predicate, and back", () => {
    const on = setMeaningOnly("author:worf risa trip", true);
    expect(on).toBe('author:worf qmd_vsearch:"risa trip"');
    expect(meaningOnly(on)).toBe(true);
    const off = setMeaningOnly(on, false);
    expect(off).toBe("author:worf risa trip");
    expect(meaningOnly(off)).toBe(false);
  });

  it("gathers bare words and a typed predicate into one", () => {
    expect(setMeaningOnly('qmd:"earl grey" hot is:document', true)).toBe(
      'is:document qmd_vsearch:"earl grey hot"',
    );
  });

  it("takes the quotes off a phrase it moves", () => {
    expect(setMeaningOnly('"earl grey" tea', true)).toBe('qmd_vsearch:"earl grey tea"');
  });

  it("has nothing to move in a query of filters alone", () => {
    expect(setMeaningOnly("is:document -kind:contact", true)).toBe("is:document -kind:contact");
    expect(meaningOnly("is:document")).toBe(false);
  });

  it("reads the last predicate that carries text, as the backend does", () => {
    expect(meaningOnly("qmd_vsearch:risa qmd:trip")).toBe(false);
    expect(meaningOnly("qmd:risa qmd_vsearch:trip")).toBe(true);
    expect(meaningOnly("risa qmd_vsearch:trip")).toBe(true);
  });
});

describe("the picked source, as the query says it", () => {
  it("is the one source_id filter, typed or clicked", () => {
    expect(pickedSource("risa source_id:slack")).toBe("slack");
    expect(pickedSource('source_id:"tng email"')).toBe("tng email");
    expect(pickedSource("risa")).toBeNull();
  });

  it("is none when the query names several, or only excludes one", () => {
    expect(pickedSource("source_id:slack source_id:tng_email")).toBeNull();
    expect(pickedSource("-source_id:slack")).toBeNull();
  });

  it("replaces every source filter and leaves the rest of the query", () => {
    expect(setSource("source_id:slack risa source_id:tng_email", "notion")).toBe(
      "risa source_id:notion",
    );
    expect(setSource("risa source_id:slack -source_id:garmin", null)).toBe(
      "risa -source_id:garmin",
    );
    expect(setSource("", "slack")).toBe("source_id:slack");
  });
});

describe("markWords", () => {
  it("marks whole words of the free text, case-blind", () => {
    expect(markWords("Off to Risa, not Risan", "author:x risa")).toEqual([
      { text: "Off to ", hit: false },
      { text: "Risa", hit: true },
      { text: ", not Risan", hit: false },
    ]);
  });
  it("marks nothing when only filters were typed", () => {
    expect(markWords("anything", "kind:chat")).toEqual([{ text: "anything", hit: false }]);
  });
  it("marks the words a meaning-only predicate carries", () => {
    expect(markWords("warp core", 'qmd_vsearch:"warp core"').filter((p) => p.hit)).toHaveLength(2);
  });
  it("finds free text among filter words", () => {
    expect(freeText("kind:chat  warp   -author:q core")).toBe("warp core");
  });
});
