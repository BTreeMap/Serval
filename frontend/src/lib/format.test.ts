import { test } from "node:test";
import assert from "node:assert/strict";
import { formatDate } from "./format.ts";

// The hoisted `Intl.DateTimeFormat` must be observationally identical to the
// per-call `toLocaleString()` it replaced. It is not identical for the obvious
// spellings — a bare `new Intl.DateTimeFormat()` yields date only, and
// `dateStyle: "short"` abbreviates the year — so this is the check that the
// explicit field set is the right one.
const INSTANTS = [
    "2026-07-28T15:33:00Z",
    "1999-01-01T00:00:00Z",
    "2026-12-31T23:59:59Z",
    "2000-02-29T12:00:00Z",
    "2026-07-04T09:05:03Z",
    "1970-01-01T00:00:00Z",
];

test("matches toLocaleString byte-for-byte in the runtime locale", () => {
    for (const iso of INSTANTS) {
        assert.equal(formatDate(iso), new Date(iso).toLocaleString(), `for ${iso}`);
    }
});

test("negative control: a different field set would be caught", () => {
    // Guards the guard — proves the assertion above can fail, rather than being
    // vacuously true because both sides call the same thing.
    const probe = new Date(INSTANTS[0]);
    const wrong = new Intl.DateTimeFormat(undefined, { dateStyle: "short" }).format(probe);
    assert.notEqual(wrong, probe.toLocaleString());
});

test("echoes an unparseable timestamp verbatim", () => {
    // Server data we cannot read is shown as-is rather than as "Invalid Date".
    assert.equal(formatDate("not a date"), "not a date");
    assert.equal(formatDate(""), "");
});
