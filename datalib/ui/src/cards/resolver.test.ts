// The one resolver's rules: a draw pass is one request, an answer is
// kept, an edit forgets and redraws, and an answer from before an edit
// is dropped.

import { describe, expect, it } from "vitest";

import { Resolver } from "./resolver";

function fake() {
  const calls: string[][] = [];
  const pending: { keys: string[]; resolve: (m: Map<string, string>) => void }[] = [];
  const errors: string[] = [];
  const r = new Resolver<string>(
    (keys) => {
      calls.push([...keys]);
      return new Promise((resolve) => pending.push({ keys, resolve }));
    },
    (e) => errors.push(e.message),
  );
  const answer = (i = pending.length - 1) =>
    pending[i].resolve(new Map(pending[i].keys.map((k) => [k, `who:${k}`])));
  return { r, calls, pending, errors, answer };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("Resolver", () => {
  it("sends every question drawn in one pass as one request, and keeps the answers", async () => {
    const { r, calls, answer } = fake();
    expect(r.lookup("a")).toBeUndefined();
    expect(r.lookup("b")).toBeUndefined();
    expect(r.lookup("a")).toBeUndefined();
    await tick();
    expect(calls).toEqual([["a", "b"]]);
    answer();
    await r.ask(["a", "b"]);
    expect(r.get("a")).toBe("who:a");
    r.lookup("a");
    await tick();
    expect(calls).toHaveLength(1);
  });

  it("tells subscribers which keys landed, and ask waits for them", async () => {
    const { r, answer } = fake();
    const told: string[][] = [];
    r.subscribe((keys) => told.push([...keys]));
    const asked = r.ask(["x"]);
    await tick();
    answer();
    await asked;
    expect(r.get("x")).toBe("who:x");
    expect(told).toEqual([["x"]]);
  });

  /** A link made in one document has to redraw a grid already showing
   *  the old answer: forgetting tells every surface, and its next draw
   *  asks again. */
  it("forgets a key on an edit, tells subscribers, and asks again on the next draw", async () => {
    const { r, calls, answer } = fake();
    await (async () => {
      const a = r.ask(["a"]);
      await tick();
      answer();
      await a;
    })();
    const told: string[][] = [];
    r.subscribe((keys) => told.push([...keys]));
    r.forget(["a"]);
    expect(told).toEqual([["a"]]);
    expect(r.lookup("a")).toBeUndefined();
    await tick();
    expect(calls).toEqual([["a"], ["a"]]);
  });

  it("drops an answer to a question asked before the edit", async () => {
    const { r, pending, answer } = fake();
    r.lookup("a");
    await tick();
    r.forget(["a"]);
    pending[0].resolve(new Map([["a", "before the edit"]]));
    await tick();
    expect(r.get("a")).toBeUndefined();
    r.lookup("a");
    await tick();
    answer(1);
    await r.ask(["a"]);
    expect(r.get("a")).toBe("who:a");
  });

  it("reports a failed question, keeps the key unknown, and asks again later", async () => {
    const errors: string[] = [];
    let fail = true;
    const r = new Resolver<string>(
      async (keys) => {
        if (fail) throw new Error("index down");
        return new Map(keys.map((k) => [k, `who:${k}`]));
      },
      (e) => errors.push(e.message),
    );
    await r.ask(["a"]);
    expect(errors).toEqual(["index down"]);
    expect(r.get("a")).toBeUndefined();
    fail = false;
    await r.ask(["a"]);
    expect(r.get("a")).toBe("who:a");
  });
});
