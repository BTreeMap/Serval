import { test } from "node:test";
import assert from "node:assert/strict";
import { ApiError } from "./api-error.ts";
import { messageOf } from "./errors.ts";

const GENERIC = "Something went wrong. Please try again.";

test("passes through a message the Control Plane wrote for a human", () => {
    assert.equal(messageOf(new ApiError(404, "snippet not found")), "snippet not found");
});

test("collapses everything else, so runtime vocabulary never reaches the UI", () => {
    // A dropped connection surfaces as a TypeError whose message ("Failed to
    // fetch", "NetworkError when attempting to fetch resource") is browser
    // trivia, not guidance.
    assert.equal(messageOf(new TypeError("Failed to fetch")), GENERIC);
    assert.equal(messageOf(new Error("kaboom")), GENERIC);
    assert.equal(messageOf("a bare string"), GENERIC);
    assert.equal(messageOf(null), GENERIC);
    assert.equal(messageOf(undefined), GENERIC);
});

test("carries the status for caller-side branching", () => {
    assert.equal(new ApiError(400, "bad cursor").status, 400);
    assert.equal(new ApiError(400, "bad cursor").name, "ApiError");
});
