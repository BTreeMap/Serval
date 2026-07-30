// Runs the test suite, refusing to succeed if it discovered nothing.
//
// `node --test` exits 0 when it finds no test files. That makes the runner a
// silent no-op after a rename, a moved directory, or a change in Node's
// discovery patterns — and since `npm run verify` chains lint && test && build,
// a vacuous test step would let every workflow and the Docker image report a
// green frontend while checking none of it. The same failure mode as an empty
// glob making every assertion below it vacuously true.
//
// So: assert the suite still exists, then hand off to the real runner. `node
// --test` is invoked with no arguments deliberately, so there is no shell glob
// for a Windows or macOS runner to expand differently.
//
// This file must NOT be named to match Node's own discovery patterns
// (`test.mjs`, `*.test.mjs`, `*-test.mjs`, `test-*.mjs`, or anything under a
// `test/` directory) or the runner picks up its own launcher and executes it as
// a test case.

import { globSync } from "node:fs";
import { spawnSync } from "node:child_process";

const files = globSync("src/**/*.test.ts");

if (files.length === 0) {
    console.error(
        "test discovery found no src/**/*.test.ts files — refusing to pass vacuously.",
    );
    process.exit(1);
}

const { status, error } = spawnSync(process.execPath, ["--test"], { stdio: "inherit" });

if (error) {
    console.error(`failed to start the test runner: ${error.message}`);
    process.exit(1);
}

process.exit(status ?? 1);
