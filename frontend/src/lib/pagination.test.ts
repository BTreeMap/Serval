import { test } from "node:test";
import assert from "node:assert/strict";
import { concatPages, cursorAfter, loadedCount, type Page } from "./pagination.ts";

const page = (items: string[], nextCursor: string | null): Page<string> => ({
    items,
    nextCursor,
});

test("concatPages preserves order across the seed and every appended page", () => {
    const seed = page(["a", "b"], "c1");
    const rest = [page(["c"], "c2"), page(["d", "e"], null)];
    // Both collections are ordered ledgers, so the fold is associative but
    // emphatically not commutative — reordering here is a visible defect.
    assert.deepEqual(concatPages(seed, rest), ["a", "b", "c", "d", "e"]);
});

test("concatPages returns the seed's own array when nothing is appended", () => {
    const seed = page(["a"], null);
    // Identity-preserving, so an unchanged list does not invalidate a memo.
    assert.equal(concatPages(seed, []), seed.items);
});

test("concatPages is empty without a seed", () => {
    assert.deepEqual(concatPages(null, []), []);
});

test("cursorAfter reads the last page loaded, not the seed", () => {
    const seed = page(["a"], "c1");
    assert.equal(cursorAfter(seed, []), "c1");
    // Reading the seed's cursor after appending would re-request a page the
    // caller already holds.
    assert.equal(cursorAfter(seed, [page(["b"], "c2")]), "c2");
    assert.equal(cursorAfter(seed, [page(["b"], null)]), null);
    assert.equal(cursorAfter(null, []), null);
});

test("loadedCount totals the seed and appended pages", () => {
    assert.equal(loadedCount(null, []), 0);
    assert.equal(loadedCount(page(["a", "b"], null), []), 2);
    assert.equal(loadedCount(page(["a", "b"], "c"), [page(["c"], null)]), 3);
});
