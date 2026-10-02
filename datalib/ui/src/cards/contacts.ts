// The contacts app as the document view sees it. A renderer writes the
// author's handle on the author span (`data-handle="email:…"`); this
// asks the `datalib_contacts` applet which contact each handle belongs
// to and turns the span into a chip. The applet is an app of its own
// (docs/dev/plans/contacts.md): with none configured, spans stay plain
// text and nothing is offered. The rules are pure and unit-tested; only
// `decorateHandles` touches a DOM.

import { iconUrl } from "@/config/icons";
import { pushToast } from "@/toasts";

export const CONTACTS_APPLET = "/applet/datalib_contacts";

export type Resolved = {
  contact_id: string;
  name: string;
  kind: string;
  /** A partial date (`2019`, `2019-06`, `2019-06-14`) by which this
   *  handle had stopped working, or null while it works. */
  stopped_working_by: string | null;
};

export type ContactSummary = { contact_id: string; name: string; kind: string };

export type Contact = ContactSummary & {
  note: string | null;
  handles: { handle: string; stopped_working_by: string | null }[];
};

// ── Pure rules ─────────────────────────────────────────────────────────

export type HandleKind = "email" | "tel" | "slack";

export function handleKind(handle: string): HandleKind | null {
  const kind = handle.slice(0, handle.indexOf(":"));
  return kind === "email" || kind === "tel" || kind === "slack" ? kind : null;
}

export function handleValue(handle: string): string {
  return handle.slice(handle.indexOf(":") + 1);
}

const KIND_ICON: Record<HandleKind, string> = { email: "email", tel: "sms", slack: "slack" };

export function handleIcon(handle: string): string | null {
  const kind = handleKind(handle);
  return kind ? KIND_ICON[kind] : null;
}

/** A name to offer for a new contact, from what the source showed:
 *  `Will Riker <riker@enterprise.org>` → `Will Riker`. Nothing when the
 *  source showed only the identifier itself. */
export function suggestedName(shownAs: string, handle: string): string {
  const name = shownAs
    .replace(/\s*<[^>]*>\s*$/, "")
    .replace(/^"(.*)"$/, "$1")
    .trim();
  return name === handleValue(handle) || name.includes("@") ? "" : name;
}

export function todayPartialDate(now: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** What the source called the handle, without the identifier it may
 *  have carried: `Will Riker <riker@enterprise.org>` → `Will Riker`. The
 *  identifier itself when that is all the source showed. */
export function sourceLabel(shownAs: string, handle: string): string {
  return suggestedName(shownAs, handle) || shownAs.trim() || handleValue(handle);
}

export type ChipLook = {
  text: string;
  ariaLabel: string;
  classes: string[];
  /** A contact's initial, drawn in a small disc; null for an
   *  unresolved handle, which shows its kind's mark instead. */
  initial: string | null;
  icon: string | null;
};

export function chipLook(handle: string, shownAs: string, r: Resolved | undefined): ChipLook {
  if (!r) {
    const text = sourceLabel(shownAs, handle);
    return {
      text,
      ariaLabel: `${text}, ${handleValue(handle)}, not linked to a contact`,
      classes: ["handle-chip", "handle-unresolved"],
      initial: null,
      icon: handleIcon(handle),
    };
  }
  return {
    text: r.name,
    ariaLabel: `${r.name}, ${handleValue(handle)}`,
    classes: ["handle-chip", "handle-resolved", ...(r.stopped_working_by ? ["handle-stale"] : [])],
    initial: [...r.name.trim()][0]?.toUpperCase() ?? "?",
    icon: null,
  };
}

export type HoverCard = {
  name: string;
  /** The handle as a person reads it, beside its kind's mark. */
  value: string;
  icon: string | null;
  lines: string[];
};

export function hoverCard(handle: string, shownAs: string, r: Resolved | null): HoverCard {
  const label = sourceLabel(shownAs, handle);
  const lines: string[] = [];
  if (r?.stopped_working_by) lines.push(`Stopped working by ${r.stopped_working_by}`);
  if (shownAs.trim() && (!r || shownAs.trim() !== r.name))
    lines.push(`Shown here as “${shownAs.trim()}”`);
  lines.push(r ? "Click to edit" : "Not linked to a contact. Click to link it.");
  return { name: r?.name ?? label, value: handleValue(handle), icon: handleIcon(handle), lines };
}

/** A chip as copied text: the name it shows and the identifier behind
 *  it, so a paste loses neither. */
export function copyText(handle: string, label: string): string {
  const value = handleValue(handle);
  if (!label || label === value) return value;
  switch (handleKind(handle)) {
    case "email":
      return `${label} <${value}>`;
    case "tel":
      return `${label} (${value})`;
    default:
      return `${label} (${handle})`;
  }
}

/** Replace every chip in a copied fragment with its copy text, keeping
 *  `data-handle` on a plain span so a paste into the app can chip it
 *  again. Mutates `fragment`; returns whether it held any chip. */
export function rewriteChipsForCopy(fragment: DocumentFragment | Element): boolean {
  const chips = Array.from(fragment.querySelectorAll<HTMLElement>(".handle-chip[data-handle]"));
  for (const chip of chips) {
    const handle = chip.dataset.handle ?? "";
    const span = chip.ownerDocument.createElement("span");
    span.dataset.handle = handle;
    span.textContent = copyText(handle, chip.dataset.label ?? "");
    chip.replaceWith(span);
  }
  return chips.length > 0;
}

/** The author spans a renderer wrote, and none a message body did.
 *  DOMPurify keeps every `data-*`, and a body is HTML a stranger wrote,
 *  so a `data-handle` counts only on the `.msg-author` in the first
 *  `h2` directly under a top-level `.msg` — the header line, which the
 *  renderer writes before the body and which a body cannot precede. */
export function trustedHandleSpans(root: Element): HTMLElement[] {
  const out: HTMLElement[] = [];
  for (const msg of root.querySelectorAll<HTMLElement>(".msg[data-section-uuid]")) {
    if (msg.parentElement?.closest(".msg")) continue;
    const header = Array.from(msg.children).find((c) => c.tagName === "H2");
    const span = header?.querySelector<HTMLElement>(":scope > span.msg-author[data-handle]");
    if (span) out.push(span);
  }
  return out;
}

// ── The applet ────────────────────────────────────────────────────────

/** Whether a failed call means the app is simply not configured. */
function isAbsent(status: number, body: string): boolean {
  return status === 502 && body.includes('no applet "datalib_contacts"');
}

async function call<T>(path: string, init?: RequestInit): Promise<T> {
  const r = await fetch(`${CONTACTS_APPLET}${path}`, {
    ...init,
    headers: init?.body ? { "content-type": "application/json" } : undefined,
  });
  const text = await r.text();
  if (!r.ok) {
    let msg = text;
    try {
      msg = (JSON.parse(text) as { error?: string }).error ?? text;
    } catch {
      // not JSON: the text itself is the message
    }
    throw new Error(msg);
  }
  return JSON.parse(text) as T;
}

const post = <T>(path: string, body: unknown) =>
  call<T>(path, { method: "POST", body: JSON.stringify(body) });

/** `null` when no contacts app is configured. */
export async function resolveHandles(handles: string[]): Promise<Record<string, Resolved> | null> {
  const r = await fetch(`${CONTACTS_APPLET}/resolve`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ handles }),
  });
  const text = await r.text();
  if (isAbsent(r.status, text)) return null;
  if (!r.ok) throw new Error(`resolve → ${r.status}: ${text}`);
  return (JSON.parse(text) as { resolved: Record<string, Resolved> }).resolved;
}

export async function searchContacts(q: string): Promise<ContactSummary[]> {
  const r = await call<{ contacts: ContactSummary[] }>(`/search?q=${encodeURIComponent(q)}`);
  return r.contacts;
}

export async function createContact(name: string, handles: string[]): Promise<string> {
  return (await post<{ contact_id: string }>("/contacts", { name, handles })).contact_id;
}

export async function fetchContact(contactId: string): Promise<Contact> {
  return call<Contact>(`/contact/${encodeURIComponent(contactId)}`);
}

export async function linkHandle(handle: string, contactId: string): Promise<void> {
  await post("/link", { handle, contact_id: contactId });
}

export async function unlinkHandle(handle: string): Promise<void> {
  await post("/unlink", { handle });
}

export async function setStoppedWorking(handle: string, by: string | null): Promise<void> {
  await post("/stopped_working", { handle, by });
}

// ── The DOM ───────────────────────────────────────────────────────────

let warnedOnce = false;

/** Resolve every trusted handle span under `root` and draw it as a
 *  chip, returning what resolved; `null` when there is nothing to draw.
 *  Safe to call again after an edit: each span keeps what the source
 *  showed in `data-shown-as`. */
export async function decorateHandles(root: HTMLElement): Promise<Record<string, Resolved> | null> {
  const spans = trustedHandleSpans(root);
  if (spans.length === 0) return null;
  for (const s of spans) {
    if (s.dataset.shownAs === undefined) s.dataset.shownAs = s.textContent ?? "";
  }
  const handles = [...new Set(spans.map((s) => s.dataset.handle ?? ""))];
  let resolved: Record<string, Resolved> | null;
  try {
    resolved = await resolveHandles(handles);
  } catch (e) {
    if (!warnedOnce) pushToast(`Contacts: ${(e as Error).message}`);
    warnedOnce = true;
    return null;
  }
  if (resolved === null) return null;
  for (const s of spans) {
    if (!s.isConnected) continue;
    const handle = s.dataset.handle ?? "";
    const look = chipLook(handle, s.dataset.shownAs ?? "", resolved[handle]);
    s.className = ["msg-author", ...look.classes].join(" ");
    s.removeAttribute("title");
    s.setAttribute("aria-label", look.ariaLabel);
    s.dataset.label = look.text;
    const lead = document.createElement(look.initial ? "span" : "img");
    lead.setAttribute("aria-hidden", "true");
    if (look.initial) {
      lead.className = "handle-initial";
      lead.textContent = look.initial;
    } else {
      const url = iconUrl(look.icon);
      if (url) (lead as HTMLImageElement).src = url;
      (lead as HTMLImageElement).alt = "";
      lead.className = "handle-mark";
    }
    s.replaceChildren(lead, document.createTextNode(look.text));
  }
  return resolved;
}

/** The selection, as a range inside `root`, or null when it is elsewhere.
 *  The document view lives in a shadow root, where Chromium answers
 *  through `ShadowRoot.getSelection` and WebKit through
 *  `Selection.getComposedRanges`. */
function selectionWithin(root: HTMLElement): Range | null {
  const host = root.getRootNode() as
    (ShadowRoot & { getSelection?: () => Selection | null }) | Document;
  const sel = (host instanceof ShadowRoot && host.getSelection?.()) || document.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
  let range: Range = sel.getRangeAt(0);
  const composed = (
    sel as Selection & {
      getComposedRanges?: (o: { shadowRoots: ShadowRoot[] }) => StaticRange[];
    }
  ).getComposedRanges;
  if (host instanceof ShadowRoot && composed && !root.contains(range.commonAncestorContainer)) {
    const [r] = composed.call(sel, { shadowRoots: [host] });
    if (!r) return null;
    range = document.createRange();
    range.setStart(r.startContainer, r.startOffset);
    range.setEnd(r.endContainer, r.endOffset);
  }
  return root.contains(range.commonAncestorContainer) ? range : null;
}

/** A copy from the document: chips become `Name <identifier>` in the
 *  plain text and keep their `data-handle` in the HTML. A selection with
 *  no chip in it is left to the browser. */
export function copyWithHandles(ev: ClipboardEvent, root: HTMLElement): void {
  const range = selectionWithin(root);
  if (!range || !ev.clipboardData) return;
  const fragment = range.cloneContents();
  if (!rewriteChipsForCopy(fragment)) return;
  // `innerText` keeps line breaks only for a laid-out element, so the
  // copy is laid out off-screen for the moment it is read.
  const holder = document.createElement("div");
  holder.style.cssText = "position:fixed;left:-99999px;top:0;width:800px";
  holder.append(fragment);
  root.append(holder);
  const text = holder.innerText;
  const html = holder.innerHTML;
  holder.remove();
  ev.clipboardData.setData("text/plain", text);
  ev.clipboardData.setData("text/html", html);
  ev.preventDefault();
}
