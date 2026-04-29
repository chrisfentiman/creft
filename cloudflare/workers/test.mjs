import { test } from "node:test";
import assert from "node:assert/strict";
import { parseUserAgent, targetTriple, buildLatestPayload, parseLatestBody } from "./creft-run.js";

// ---------------------------------------------------------------------------
// parseUserAgent
// ---------------------------------------------------------------------------

test("parseUserAgent parses a well-formed creft UA", () => {
  const result = parseUserAgent("creft/0.5.1 (darwin; aarch64)");
  assert.deepEqual(result, { version: "0.5.1", os: "darwin", arch: "aarch64" });
});

test("parseUserAgent parses linux x86_64 UA", () => {
  const result = parseUserAgent("creft/1.2.3 (linux; x86_64)");
  assert.deepEqual(result, { version: "1.2.3", os: "linux", arch: "x86_64" });
});

test("parseUserAgent returns empty strings for curl UA", () => {
  const result = parseUserAgent("curl/8.4.0");
  assert.deepEqual(result, { version: "", os: "", arch: "" });
});

test("parseUserAgent returns empty strings for null UA", () => {
  const result = parseUserAgent(null);
  assert.deepEqual(result, { version: "", os: "", arch: "" });
});

test("parseUserAgent returns empty strings for empty string UA", () => {
  const result = parseUserAgent("");
  assert.deepEqual(result, { version: "", os: "", arch: "" });
});

test("parseUserAgent returns empty strings for UA with extra fields", () => {
  // Extra semicolon-delimited fields break the strict pattern.
  const result = parseUserAgent("creft/0.5.1 (darwin; aarch64; extra)");
  assert.deepEqual(result, { version: "", os: "", arch: "" });
});

test("parseUserAgent returns empty strings for browser UA", () => {
  const result = parseUserAgent(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
  );
  assert.deepEqual(result, { version: "", os: "", arch: "" });
});

// ---------------------------------------------------------------------------
// targetTriple
// ---------------------------------------------------------------------------

test("targetTriple maps darwin aarch64 to aarch64-apple-darwin", () => {
  assert.equal(targetTriple("darwin", "aarch64"), "aarch64-apple-darwin");
});

test("targetTriple maps darwin x86_64 to x86_64-apple-darwin", () => {
  assert.equal(targetTriple("darwin", "x86_64"), "x86_64-apple-darwin");
});

test("targetTriple maps linux x86_64 to x86_64-unknown-linux-gnu", () => {
  assert.equal(targetTriple("linux", "x86_64"), "x86_64-unknown-linux-gnu");
});

test("targetTriple maps linux aarch64 to aarch64-unknown-linux-gnu", () => {
  assert.equal(targetTriple("linux", "aarch64"), "aarch64-unknown-linux-gnu");
});

test("targetTriple returns null for unsupported OS", () => {
  assert.equal(targetTriple("windows", "x86_64"), null);
});

test("targetTriple returns null for unsupported arch", () => {
  assert.equal(targetTriple("linux", "arm"), null);
});

test("targetTriple returns null for empty strings", () => {
  assert.equal(targetTriple("", ""), null);
});

// ---------------------------------------------------------------------------
// buildLatestPayload
// ---------------------------------------------------------------------------

test("buildLatestPayload includes platform URLs for a known target triple", () => {
  const payload = buildLatestPayload(
    "0.5.1",
    "creft-v0.5.1",
    "creft/0.5.1 (darwin; aarch64)"
  );
  assert.equal(payload.version, "0.5.1");
  assert.equal(payload.tag, "creft-v0.5.1");
  assert.equal(
    payload.tarball_url,
    "https://github.com/chrisfentiman/creft/releases/download/creft-v0.5.1/creft-aarch64-apple-darwin.tar.gz"
  );
  assert.equal(
    payload.checksum_url,
    "https://github.com/chrisfentiman/creft/releases/download/creft-v0.5.1/creft-aarch64-apple-darwin.tar.gz.sha256"
  );
});

test("buildLatestPayload tarball_url matches install script asset path pattern", () => {
  // scripts/install.sh:213 builds:
  // https://github.com/chrisfentiman/creft/releases/download/${tag}/${tarball_name}
  // where tarball_name = "creft-${target}.tar.gz"
  const payload = buildLatestPayload(
    "0.5.1",
    "creft-v0.5.1",
    "creft/0.5.1 (linux; x86_64)"
  );
  const expected =
    "https://github.com/chrisfentiman/creft/releases/download/creft-v0.5.1/creft-x86_64-unknown-linux-gnu.tar.gz";
  assert.equal(payload.tarball_url, expected);
});

test("buildLatestPayload returns only version and tag when UA does not parse", () => {
  const payload = buildLatestPayload("0.5.1", "creft-v0.5.1", "curl/8.4.0");
  assert.equal(payload.version, "0.5.1");
  assert.equal(payload.tag, "creft-v0.5.1");
  assert.equal(Object.hasOwn(payload, "tarball_url"), false);
  assert.equal(Object.hasOwn(payload, "checksum_url"), false);
});

test("buildLatestPayload returns only version and tag when UA is empty", () => {
  const payload = buildLatestPayload("0.5.1", "creft-v0.5.1", "");
  assert.equal(payload.version, "0.5.1");
  assert.equal(payload.tag, "creft-v0.5.1");
  assert.equal(Object.hasOwn(payload, "tarball_url"), false);
  assert.equal(Object.hasOwn(payload, "checksum_url"), false);
});

test("buildLatestPayload returns only version and tag for unsupported platform", () => {
  const payload = buildLatestPayload(
    "0.5.1",
    "creft-v0.5.1",
    "creft/0.5.1 (windows; x86_64)"
  );
  assert.equal(payload.version, "0.5.1");
  assert.equal(payload.tag, "creft-v0.5.1");
  assert.equal(Object.hasOwn(payload, "tarball_url"), false);
  assert.equal(Object.hasOwn(payload, "checksum_url"), false);
});

// ---------------------------------------------------------------------------
// parseLatestBody
// ---------------------------------------------------------------------------

test("parseLatestBody returns parsed object for valid JSON", () => {
  const body = JSON.stringify({ tag_name: "creft-v0.5.1", name: "creft 0.5.1" });
  const result = parseLatestBody(body);
  assert.equal(result.tag_name, "creft-v0.5.1");
});

test("parseLatestBody throws a 502 Response for malformed JSON", () => {
  let thrown;
  try {
    parseLatestBody("<!DOCTYPE html><html>maintenance</html>");
  } catch (err) {
    thrown = err;
  }
  assert.ok(thrown instanceof Response, "must throw a Response, not a SyntaxError");
  assert.equal(thrown.status, 502);
  assert.equal(thrown.headers.get("cache-control"), "no-store");
});

test("parseLatestBody throws a 502 Response for an empty body", () => {
  let thrown;
  try {
    parseLatestBody("");
  } catch (err) {
    thrown = err;
  }
  assert.ok(thrown instanceof Response, "must throw a Response, not a SyntaxError");
  assert.equal(thrown.status, 502);
  assert.equal(thrown.headers.get("cache-control"), "no-store");
});

test("parseLatestBody throws a 502 Response for a truncated JSON body", () => {
  let thrown;
  try {
    parseLatestBody('{"tag_name":"creft-v0.5.1"');
  } catch (err) {
    thrown = err;
  }
  assert.ok(thrown instanceof Response, "must throw a Response, not a SyntaxError");
  assert.equal(thrown.status, 502);
});
