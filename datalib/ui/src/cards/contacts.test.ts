// The chip rules: which spans count, what a chip says, and the name a
// new contact is offered.

import { describe, expect, it } from "vitest";

import { chipLook, suggestedName, todayPartialDate, trustedHandleSpans } from "./contacts";

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
  it("shows the source's text and the kind's mark when unresolved", () => {
    const look = chipLook(
      "email:riker@enterprise.org",
      "Will Riker <riker@enterprise.org>",
      undefined,
    );
    expect(look.text).toBe("Will Riker <riker@enterprise.org>");
    expect(look.icon).toBe("email");
    expect(look.classes).toContain("handle-unresolved");
  });

  it("shows the contact's name when resolved, and says when the handle stopped working", () => {
    const r = { contact_id: "c", name: "Will Riker", kind: "person", stopped_working_by: "2019" };
    const look = chipLook("tel:+15550123456", "+1 555 012 3456", r);
    expect(look.text).toBe("Will Riker");
    expect(look.initial).toBe("W");
    expect(look.classes).toContain("handle-stale");
    expect(look.title).toContain("stopped working by 2019");
    expect(look.title).toContain("+1 555 012 3456");
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
