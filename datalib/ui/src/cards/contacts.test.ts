// The chip rules: which spans count, what a chip and its hover card say,
// what a copy carries, and the name a new contact is offered.

import { describe, expect, it } from "vitest";

import {
  chipLook,
  copyText,
  hoverCard,
  rewriteChipsForCopy,
  suggestedName,
  todayPartialDate,
  trustedHandleSpans,
} from "./contacts";

function dom(html: string): HTMLElement {
  const div = document.createElement("div");
  div.innerHTML = html;
  return div;
}

const header = (handle: string, name: string) =>
  `<h2><span class="msg-author" data-handle="${handle}">${name}</span> <time class="msg-ts">x</time></h2>`;

describe("trustedHandleSpans", () => {
  it("takes the header's author span", () => {
    const root = dom(
      `<div id="m-1" data-section-uuid="1" class="msg">${header("email:riker@enterprise.org", "Will Riker")}<p>Number One.</p></div>`,
    );
    const spans = trustedHandleSpans(root);
    expect(spans.map((s) => s.dataset.handle)).toEqual(["email:riker@enterprise.org"]);
  });

  /** A message body is HTML its sender wrote; a `data-handle` in it must
   *  never become a chip naming one of the reader's contacts. */
  it("ignores a data-handle a message body wrote", () => {
    const forged = header("email:picard@enterprise.org", "Captain Picard");
    const root = dom(
      `<div id="m-1" data-section-uuid="1" class="msg">${header("email:q@continuum.org", "Q")}` +
        `<p>${forged}</p>${forged}` +
        `<div id="m-2" data-section-uuid="2" class="msg">${forged}</div></div>` +
        // A header with no author: the body's span is not the header's.
        `<div id="m-3" data-section-uuid="3" class="msg"><h2><time>x</time></h2>${forged}</div>`,
    );
    expect(trustedHandleSpans(root).map((s) => s.dataset.handle)).toEqual([
      "email:q@continuum.org",
    ]);
  });
});

describe("chipLook", () => {
  it("shows the source's name without the address, and the kind's mark, when unresolved", () => {
    const look = chipLook(
      "email:riker@enterprise.org",
      "Will Riker <riker@enterprise.org>",
      undefined,
    );
    expect(look.text).toBe("Will Riker");
    expect(look.icon).toBe("email");
    expect(look.classes).toContain("handle-unresolved");
    expect(chipLook("tel:+15550123456", "+15550123456", undefined).text).toBe("+15550123456");
  });

  it("shows the contact's name when resolved, faded when the handle stopped working", () => {
    const r = { contact_id: "c", name: "Will Riker", kind: "person", stopped_working_by: "2019" };
    const look = chipLook("tel:+15550123456", "+1 555 012 3456", r);
    expect(look.text).toBe("Will Riker");
    expect(look.initial).toBe("W");
    expect(look.classes).toContain("handle-stale");
  });
});

describe("hoverCard", () => {
  it("names the identifier behind the short name, and how the message showed it", () => {
    const r = { contact_id: "c", name: "Will Riker", kind: "person", stopped_working_by: "2019" };
    const card = hoverCard("tel:+15550123456", "+1 555 012 3456", r);
    expect(card.name).toBe("Will Riker");
    expect(card.value).toBe("+15550123456");
    expect(card.icon).toBe("sms");
    expect(card.lines).toEqual([
      "Stopped working by 2019",
      "Shown here as “+1 555 012 3456”",
      "Click to edit",
    ]);
  });

  it("says an unlinked handle can be linked", () => {
    const card = hoverCard("email:q@continuum.org", "Q <q@continuum.org>", null);
    expect(card.name).toBe("Q");
    expect(card.lines.at(-1)).toContain("Not linked");
  });
});

describe("copy", () => {
  it("carries the name and the identifier", () => {
    expect(copyText("email:riker@enterprise.org", "Will Riker")).toBe(
      "Will Riker <riker@enterprise.org>",
    );
    expect(copyText("tel:+15550123456", "Will Riker")).toBe("Will Riker (+15550123456)");
    expect(copyText("slack:T1/U2", "Data")).toBe("Data (slack:T1/U2)");
    expect(copyText("tel:+15550123456", "+15550123456")).toBe("+15550123456");
  });

  /** A copied chip once came out as "WWill Riker": the initial disc's
   *  letter and the name, with the address gone. */
  it("replaces each chip in a selection, disc and all, keeping data-handle", () => {
    const root = dom(
      `<p>From <span class="msg-author handle-chip handle-resolved" data-handle="email:riker@enterprise.org" data-label="Will Riker"><span class="handle-initial">W</span>Will Riker</span>, hello</p>`,
    );
    expect(rewriteChipsForCopy(root)).toBe(true);
    expect(root.textContent).toBe("From Will Riker <riker@enterprise.org>, hello");
    expect(root.querySelector("[data-handle]")?.getAttribute("data-handle")).toBe(
      "email:riker@enterprise.org",
    );
    expect(rewriteChipsForCopy(dom("<p>no chips</p>"))).toBe(false);
  });
});

describe("suggestedName", () => {
  it("takes the display name and leaves a bare identifier blank", () => {
    expect(suggestedName("Will Riker <riker@enterprise.org>", "email:riker@enterprise.org")).toBe(
      "Will Riker",
    );
    expect(
      suggestedName('"Riker, Will" <riker@enterprise.org>', "email:riker@enterprise.org"),
    ).toBe("Riker, Will");
    expect(suggestedName("riker@enterprise.org", "email:riker@enterprise.org")).toBe("");
    expect(suggestedName("Lt. Cmdr. Data", "slack:T1/U2")).toBe("Lt. Cmdr. Data");
  });
});

describe("todayPartialDate", () => {
  it("is a full local date", () => {
    expect(todayPartialDate(new Date(2026, 0, 5))).toBe("2026-01-05");
  });
});
