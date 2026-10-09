// The search field's CodeMirror extensions. The document is the query
// text; a value naming a source, a group or a step is drawn over as its
// chip, and the menu offers keys and values from the table's `…/keys`
// and `…/values` (docs/dev/plans/search_autocomplete.md § "The field").
import {
  acceptCompletion,
  autocompletion,
  completionStatus,
  selectedCompletionIndex,
  setSelectedCompletion,
  startCompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import {
  EditorState,
  Prec,
  StateEffect,
  StateField,
  type Extension,
  type Transaction,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { fetchSearchKeys, fetchSearchValues, type KeyValues, type SearchKeySpec } from "@/api";
import { entities, entityCell, type EntityView } from "@/cards/entities";
import { chipUri, chipWords, completingAt, keyNamed, termValue, words } from "./queryText";

export const setKeys = StateEffect.define<SearchKeySpec[]>();

/// Each table's keys, asked once per page; a failure is asked again next
/// time, and offers no keys meanwhile.
const keysAsked = new Map<string, Promise<SearchKeySpec[]>>();
export function keysOf(base: string): Promise<SearchKeySpec[]> {
  let asked = keysAsked.get(base);
  if (!asked) {
    asked = fetchSearchKeys(base).catch(() => {
      keysAsked.delete(base);
      return [];
    });
    keysAsked.set(base, asked);
  }
  return asked;
}

const keysField = StateField.define<SearchKeySpec[]>({
  create: () => [],
  update: (keys, tr) => tr.effects.find((e) => e.is(setKeys))?.value ?? keys,
});

/// The word being typed is drawn as text until the cursor leaves it, so
/// `source_id:sla` does not turn into a chip for a source named "sla".
type Span = { from: number; to: number };

function onlySpace(tr: Transaction): boolean {
  let spaces = true;
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    const text = tr.startState.sliceDoc(fromA, toA) + inserted.toString();
    if (/\S/.test(text)) spaces = false;
  });
  return spaces;
}

const editingField = StateField.define<Span | null>({
  create: () => null,
  update(span, tr) {
    const head = tr.state.selection.main.head;
    // A pick from the menu is finished: its chip shows at once.
    if (tr.isUserEvent("input.complete")) return null;
    // Typing or deleting in a word opens it; a space typed or deleted
    // beside one does not, so Backspace past the space after a chip
    // reaches the chip whole.
    if (tr.docChanged && (tr.isUserEvent("input") || tr.isUserEvent("delete"))) {
      if (onlySpace(tr)) return null;
      const w = words(tr.state.doc.toString()).find((w) => w.from <= head && head <= w.to);
      return w ? { from: w.from, to: w.to } : null;
    }
    if (!span) return null;
    const mapped = tr.docChanged
      ? { from: tr.changes.mapPos(span.from, -1), to: tr.changes.mapPos(span.to, 1) }
      : span;
    return mapped.from <= head && head <= mapped.to ? mapped : null;
  },
});

const redraw = StateEffect.define<null>();

class ChipWidget extends WidgetType {
  constructor(
    readonly uri: string,
    readonly shown: string,
    readonly view: EntityView | undefined,
  ) {
    super();
  }
  eq(other: ChipWidget): boolean {
    return other.uri === this.uri && other.shown === this.shown && other.view === this.view;
  }
  toDOM(): HTMLElement {
    const a = entityCell(this.uri, this.shown, this.view, null);
    // A chip's href is never followed (docs/dev/chips.md § "Clicks").
    a.addEventListener("click", (e) => e.preventDefault());
    a.draggable = false;
    return a;
  }
}

function chipDecorations(state: EditorState): DecorationSet {
  const editing = state.field(editingField);
  const ranges = chipWords(state.doc.toString(), state.field(keysField))
    .filter(({ word }) => !editing || word.to < editing.from || word.from > editing.to)
    .map(({ word, uri }) =>
      Decoration.replace({
        widget: new ChipWidget(uri, word.value, entities.lookup(uri)),
      }).range(word.valueFrom, word.to),
    );
  return Decoration.set(ranges);
}

const chips = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    private readonly unsubscribe: () => void;
    constructor(view: EditorView) {
      this.decorations = chipDecorations(view.state);
      this.unsubscribe = entities.subscribe((keys) => {
        const shown = chipWords(view.state.doc.toString(), view.state.field(keysField));
        if (shown.some(({ uri }) => keys.has(uri))) view.dispatch({ effects: redraw.of(null) });
      });
    }
    update(u: ViewUpdate) {
      const asked = u.transactions.some((t: Transaction) =>
        t.effects.some((e) => e.is(redraw) || e.is(setKeys)),
      );
      if (u.docChanged || u.selectionSet || asked) this.decorations = chipDecorations(u.state);
    }
    destroy() {
      this.unsubscribe();
    }
  },
  {
    decorations: (p) => p.decorations,
    provide: (p) =>
      EditorView.atomicRanges.of((view) => view.plugin(p)?.decorations ?? Decoration.none),
  },
);

/// What a key's values are, in the menu beside its name.
function describe(values: KeyValues): string {
  switch (values.kind) {
    case "words":
      return (values.words ?? []).join(" · ");
    case "stamp":
      return "a date";
    case "text":
      return "";
    default:
      return values.kind;
  }
}

/// The menu: keys while a key is typed, then that key's values.
function completions(base: () => string) {
  return async (ctx: CompletionContext): Promise<CompletionResult | null> => {
    const query = ctx.state.doc.toString();
    const at = completingAt(query, ctx.pos);
    if (!at) return null;
    const keys = ctx.state.field(keysField);
    if (at.kind === "key") {
      const options: Completion[] = keys.map((k) => ({
        label: `${k.key}:`,
        detail: describe(k.values),
        type: "keyword",
        apply: (view, _c, from, to) => {
          const insert = `${k.key}:`;
          view.dispatch({
            changes: { from, to, insert },
            selection: { anchor: from + insert.length },
            userEvent: "input.complete",
          });
          if (k.values.kind !== "stamp") startCompletion(view);
        },
      }));
      return { from: at.from, to: at.to, options, validFor: /^[\w.]*$/ };
    }
    const spec = keyNamed(keys, at.key);
    if (!spec || spec.values.kind === "stamp") return null;
    let values;
    try {
      values = await fetchSearchValues(base(), spec.key, at.typed, at.rest);
    } catch {
      return null;
    }
    if (ctx.aborted || values.length === 0) return null;
    const uris = values.map((v) => chipUri(spec.values, v.value));
    await entities.ask(uris.filter((u): u is string => u !== null));
    const atEnd = at.to >= query.length;
    const options: ChipCompletion[] = values.map((v, i) => ({
      label: v.value,
      uri: uris[i] ?? undefined,
      detail: v.count === undefined ? undefined : v.count.toLocaleString(),
      apply: termValue(v.value) + (atEnd ? " " : ""),
    }));
    return { from: at.from, to: at.to, options, filter: false };
  };
}

type ChipCompletion = Completion & { uri?: string };

/// A suggestion naming a source, a group or a step, drawn as the chip the
/// field will show once it is picked.
const chipOption = {
  position: 45,
  render: (c: ChipCompletion): Node | null => {
    if (!c.uri) return null;
    const a = entityCell(c.uri, c.label, entities.get(c.uri), null);
    a.addEventListener("click", (e) => e.preventDefault());
    return a;
  },
};

/// Tab takes the chosen suggestion, or the first when none is chosen; with
/// no menu open it moves focus as Tab always does.
function takeSuggestion(view: EditorView): boolean {
  if (completionStatus(view.state) !== "active") return false;
  if (selectedCompletionIndex(view.state) === null) {
    view.dispatch({ effects: setSelectedCompletion(0) });
  }
  // With the menu open, Tab is the menu's even when it takes nothing.
  acceptCompletion(view);
  return true;
}

/// One line: a newline pasted or typed becomes a space.
const oneLine = EditorState.transactionFilter.of((tr) => {
  if (!tr.docChanged || tr.newDoc.lines === 1) return tr;
  const changes = [...tr.newDoc.toString().matchAll(/\n/g)].map((m) => ({
    from: m.index,
    to: m.index + 1,
    insert: " ",
  }));
  return [tr, { changes, sequential: true }];
});

const theme = EditorView.theme({
  "&": { flex: "1 1 auto", minWidth: "0", color: "inherit", background: "transparent" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "inherit",
    lineHeight: "inherit",
    overflowX: "auto",
    overflowY: "hidden",
    scrollbarWidth: "none",
  },
  ".cm-content": { padding: "0", caretColor: "currentColor" },
  ".cm-line": { padding: "0" },
  ".cm-placeholder": { color: "var(--datalib-faint)" },
  ".cm-tooltip": {
    background: "var(--datalib-bg)",
    color: "var(--datalib-fg)",
    border: "1px solid var(--datalib-border)",
    borderRadius: "var(--datalib-radius)",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul": {
    fontFamily: "inherit",
    fontSize: "var(--datalib-font-size)",
    maxHeight: "20em",
    maxWidth: "min(40em, 90vw)",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li": {
    display: "flex",
    alignItems: "center",
    gap: "6px",
    padding: "3px 10px",
    overflow: "hidden",
    textOverflow: "ellipsis",
  },
  ".cm-completionLabel": { overflow: "hidden", textOverflow: "ellipsis" },
  ".cm-chip-option .cm-completionLabel": { display: "none" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": {
    background: "var(--datalib-accent)",
    color: "var(--datalib-accent-fg, #fff)",
  },
  ".cm-completionDetail": {
    marginLeft: "auto",
    paddingLeft: "1.5em",
    fontStyle: "normal",
    opacity: "0.7",
    fontVariantNumeric: "tabular-nums",
  },
});

export type FieldHooks = {
  /** The table's search, whose `/keys` and `/values` the menu asks. */
  base: () => string;
  /** Enter with no suggestion chosen. */
  onSubmit: () => void;
  /** Escape with no menu open. */
  onEscape: () => void;
};

export function fieldExtensions(hooks: FieldHooks): Extension[] {
  return [
    keysField,
    editingField,
    chips,
    history(),
    oneLine,
    theme,
    autocompletion({
      override: [completions(hooks.base)],
      selectOnOpen: false,
      icons: false,
      closeOnBlur: true,
      // Nothing is chosen when the menu opens, so Enter cannot take a
      // pick by accident; Tab right after typing takes the first.
      interactionDelay: 0,
      addToOptions: [chipOption],
      // A source, group or step is drawn as its chip instead of its label.
      optionClass: (c: ChipCompletion) => (c.uri ? "cm-chip-option" : ""),
    }),
    Prec.highest(keymap.of([{ key: "Tab", run: takeSuggestion }])),
    Prec.high(
      keymap.of([
        { key: "Enter", run: () => (hooks.onSubmit(), true) },
        // Told, and passed on: a dialog the field is in still closes.
        { key: "Escape", run: () => (hooks.onEscape(), false) },
      ]),
    ),
    keymap.of([...defaultKeymap, ...historyKeymap]),
    EditorView.contentAttributes.compute(["doc"], (s) => ({ "data-query": s.doc.toString() })),
  ];
}
