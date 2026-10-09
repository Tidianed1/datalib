// What a chip in the search field does beyond what it does everywhere:
// its right-click menu is the chip's own (`entityMenu`) after the
// field's entries, and the edits those entries make to the query.
import { entityMenu, type EntityMenuEntry, type EntityMenuId } from "@/cards/entities";
import type { Word } from "./queryText";

export type FieldMenuId = "edit" | "toggle-negate" | EntityMenuId;
export type FieldMenuEntry = { id: FieldMenuId; label: string; separator?: boolean };

/** The menu on a chip in the field: edit it as text, exclude or include
 *  what it names, then everything the chip offers anywhere. */
export function fieldChipMenu(uri: string, name: string, negate: boolean): FieldMenuEntry[] {
  const own: FieldMenuEntry[] = [
    { id: "edit", label: "Edit as text" },
    { id: "toggle-negate", label: negate ? `Include ${name} instead` : `Exclude ${name}` },
  ];
  const chip: EntityMenuEntry[] = entityMenu(uri, name);
  if (chip.length > 0) chip[0] = { ...chip[0], separator: true };
  return [...own, ...chip];
}

/** The change that excludes what a term matches, or includes it again:
 *  its leading `-`. */
export function toggleNegate(word: Word): { from: number; to: number; insert: string } {
  return word.negate
    ? { from: word.from, to: word.from + 1, insert: "" }
    : { from: word.from, to: word.from, insert: "-" };
}
