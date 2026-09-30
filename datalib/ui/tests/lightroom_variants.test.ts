// A Lightroom catalog (or one backup .zip) and a folder of Lightroom's
// backups are one `lightroom` step type with two descriptors, told apart
// by which method table the step carries.
import { describe, expect, it } from "vitest";
import { CATALOG, catalogForStep } from "../src/config/catalog";
import { buildStep, listSteps, seedFieldValues } from "../src/config/sourceSteps";
import methods from "../src/config/ingestMethods.json";

const LIGHTROOM = CATALOG.filter((e) => e.type === "lightroom");
const byKey = (k: string) => LIGHTROOM.find((e) => e.variantKey === k)!;

describe("the catalog's lightroom variants", () => {
  it("has a form for every method the backend declares", () => {
    const declared = (methods as Record<string, { path: string }[]>).lightroom.map((m) => m.path);
    expect(LIGHTROOM.map((e) => e.variantKey).sort()).toEqual([...declared].sort());
    expect(LIGHTROOM.every((e) => e.wizard)).toBe(true);
  });

  it("picks a catalog file, zip included, or a backups folder", () => {
    const path = (key: string) => byKey(key).fields!.find((f) => f.kind === "path")!;
    const catalog = path("catalog");
    expect(catalog.kind === "path" && catalog.picks).toBe("file");
    expect(catalog.kind === "path" && catalog.extensions).toEqual(["lrcat", "zip"]);
    const backups = path("backups");
    expect(backups.kind === "path" && backups.picks).toBe("dir");
    expect(backups.target).toBe("backups.path");
  });

  it("writes each method under its own table and reads it back to its form", () => {
    const write = (key: string, values: Record<string, unknown>) =>
      buildStep({
        entry: byKey(key),
        group: key,
        phase: "download",
        values: { ...seedFieldValues(byKey(key)), ...values },
      });
    const backups = write("backups", { "backups.path": "~/Pictures/Lightroom/Backups" });
    expect(backups).toContain("[steps.params.backups]");
    expect(backups).toContain('path = "~/Pictures/Lightroom/Backups"');

    const steps = listSteps(`data_root = "~/datalib"

[[groups]]
id = "c"
type = "lightroom"

[[groups]]
id = "b"
type = "lightroom"

[[steps]]
group = "c"
function = "ingest"
[steps.params.catalog]
path = "~/Pictures/Lightroom/Enterprise.lrcat"

[[steps]]
group = "b"
function = "ingest"
[steps.params.backups]
path = "~/Pictures/Lightroom/Backups"
`);
    for (const [id, label] of [
      ["c/ingest", "Lightroom"],
      ["b/ingest", "Lightroom backups"],
    ]) {
      const step = steps.find((s) => s.id === id)!;
      expect(catalogForStep("lightroom", step.params)!.label, id).toBe(label);
    }
  });
});
