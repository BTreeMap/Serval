import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// Files under `public/` are copied byte-for-byte into `dist/` and are therefore
// unverified by definition: nothing parses them at build time, so a typecheck, a
// lint and a build can all be green while the shipped asset is broken. Anything
// declared in `index.html` and served verbatim needs its own check.

const root = join(import.meta.dirname, "..");
const read = (path: string) => readFileSync(join(root, path), "utf8");

const FAVICON = "public/favicon.svg";

test("the favicon exists and is not empty", () => {
    // Guards the guard: without this, a missing or emptied file would make every
    // assertion below vacuously true.
    assert.ok(read(FAVICON).trim().length > 0);
});

test("the favicon is a single well-formed svg root", () => {
    const svg = read(FAVICON).trim();
    assert.match(svg, /^<svg\s/);
    assert.ok(svg.endsWith("</svg>"), "must close its root element");
    assert.equal(svg.match(/<svg[\s>]/g)?.length, 1, "exactly one root element");
});

test("no comment in the favicon contains a double hyphen", () => {
    // `--` inside an XML comment is illegal and makes browsers render a parser
    // error page in place of the icon — with every build check still green.
    // A CSS custom property pasted into a comment is the way this happens.
    for (const [, body] of read(FAVICON).matchAll(/<!--([\s\S]*?)-->/g)) {
        assert.ok(!body.includes("--"), `illegal "--" inside comment: ${body.trim()}`);
    }
});

test("every local asset index.html declares is actually present", () => {
    const html = read("index.html");
    const hrefs = [...html.matchAll(/(?:href|src)="(\/[^"]+)"/g)].map(([, h]) => h);
    // The entry module is served by Vite from src/, not from public/.
    const assets = hrefs.filter((h) => !h.startsWith("/src/"));
    assert.ok(assets.length > 0, "expected at least one declared public asset");
    assert.ok(assets.includes("/favicon.svg"), "the favicon link must not be dropped");
    for (const asset of assets) {
        assert.ok(read(`public${asset}`).length > 0, `${asset} is declared but missing`);
    }
});
