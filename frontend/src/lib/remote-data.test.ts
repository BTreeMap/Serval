import { test } from "node:test";
import assert from "node:assert/strict";
import {
    LOADING,
    assertNever,
    failure,
    foldRemote,
    messageOfRemote,
    success,
    valueOf,
    type RemoteData,
} from "./remote-data.ts";

const cases = {
    loading: (): string => "loading",
    success: (value: number): string => `success:${value}`,
    failure: (message: string, stale: number | null): string => `failure:${message}:${stale}`,
};

test("foldRemote eliminates every variant", () => {
    assert.equal(foldRemote<number, string>(LOADING, cases), "loading");
    assert.equal(foldRemote(success(7), cases), "success:7");
    assert.equal(foldRemote(failure<number>("boom"), cases), "failure:boom:null");
    assert.equal(foldRemote(failure("gone", 3), cases), "failure:gone:3");
});

test("valueOf yields the value a success holds and the value a failure retains", () => {
    assert.equal(valueOf<number>(LOADING), null);
    assert.equal(valueOf(success(7)), 7);
    // The whole point of `stale`: a failed revalidation keeps the last list on
    // screen under the error banner instead of blanking it.
    assert.equal(valueOf(failure("boom", 7)), 7);
    assert.equal(valueOf(failure<number>("boom")), null);
});

test("messageOfRemote is set only for failures", () => {
    assert.equal(messageOfRemote<number>(LOADING), null);
    assert.equal(messageOfRemote(success(7)), null);
    assert.equal(messageOfRemote(failure<number>("boom")), "boom");
});

test("LOADING is a shared frozen reference, so it cannot defeat memoization", () => {
    const a: RemoteData<number> = LOADING;
    const b: RemoteData<string> = LOADING;
    assert.equal(a, b);
    assert.ok(Object.isFrozen(LOADING));
});

test("assertNever rejects a value that crossed an untyped boundary", () => {
    assert.throws(() => assertNever("surprise" as never), TypeError);
});
