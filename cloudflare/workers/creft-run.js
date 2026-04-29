// This file is a template. The deploy skill (creft deploy install-script)
// generates the actual worker by splitting at the marker comment below
// and injecting the contents of scripts/install.sh.
// Do not deploy this file directly — use `creft deploy install-script`.

const INSTALL_SCRIPT = `
// __INSTALL_SCRIPT_CONTENT__
`;

const VERSION_PATTERN = /^\/v?(\d+\.\d+\.\d+)$/;

// Parse "creft/0.5.1 (darwin; aarch64)" into { version, os, arch }.
// Returns { version: "", os: "", arch: "" } if the UA does not match.
function parseUserAgent(ua) {
  if (!ua) return { version: "", os: "", arch: "" };
  const m = ua.match(/^creft\/(\d+\.\d+\.\d+) \(([^;)]+); ([^;)]+)\)$/);
  if (!m) return { version: "", os: "", arch: "" };
  return { version: m[1], os: m[2].trim(), arch: m[3].trim() };
}

// Map (os, arch) to the Rust target triple used by GitHub release assets.
// Returns null for unsupported combinations.
function targetTriple(os, arch) {
  if (os === "darwin" && arch === "aarch64") return "aarch64-apple-darwin";
  if (os === "darwin" && arch === "x86_64") return "x86_64-apple-darwin";
  if (os === "linux" && arch === "x86_64") return "x86_64-unknown-linux-gnu";
  if (os === "linux" && arch === "aarch64") return "aarch64-unknown-linux-gnu";
  return null;
}

// Build the JSON response body for /latest, including platform-specific URLs
// when the UA parses to a known target triple.
function buildLatestPayload(version, tag, ua) {
  const parsed = parseUserAgent(ua);
  const triple = targetTriple(parsed.os, parsed.arch);
  const base = { version, tag };
  if (!triple) return base;
  const base_url = `https://github.com/chrisfentiman/creft/releases/download/${tag}/creft-${triple}.tar.gz`;
  return {
    ...base,
    tarball_url: base_url,
    checksum_url: `${base_url}.sha256`,
  };
}

// Log one Analytics Engine event with uniform blob shape.
// kind ∈ {"install", "latest"}. version is supplied by the caller because the
// source differs by route. os and arch are parsed from the UA — empty strings
// for install-script callers whose UA does not match the creft UA shape.
// Writes with indexes: [kind].
function logEvent(env, kind, version, ua, path) {
  if (!env.CREFT_ANALYTICS) return;
  const { os, arch } = parseUserAgent(ua);
  env.CREFT_ANALYTICS.writeDataPoint({
    blobs: [kind, version, os, arch, path],
    doubles: [1],
    indexes: [kind],
  });
}

// Resolve the latest version via the GitHub Releases API, with caches.default
// fronting the upstream call. Authenticates via env.GITHUB_TOKEN when bound.
// Returns the parsed JSON body, or throws a Response on upstream failure.
async function resolveLatest(env) {
  const cache = caches.default;
  const cacheKey = new Request("https://creft.internal/gh-releases-latest");
  let cached = await cache.match(cacheKey);
  if (cached) {
    return cached.json();
  }

  const upstreamHeaders = {
    "user-agent": "creft.run-worker",
    accept: "application/vnd.github+json",
  };
  if (env.GITHUB_TOKEN) {
    upstreamHeaders.authorization = `Bearer ${env.GITHUB_TOKEN}`;
  } else {
    console.warn("GITHUB_TOKEN not bound; falling back to anonymous upstream call");
  }

  const upstream = await fetch(
    "https://api.github.com/repos/chrisfentiman/creft/releases/latest",
    { headers: upstreamHeaders }
  );

  if (!upstream.ok) {
    throw new Response("upstream error", {
      status: 502,
      headers: { "cache-control": "no-store" },
    });
  }

  // Read body to text once. Cache a Response built from the string so the put
  // and the parse both consume the same already-materialized value — ordering
  // between cache.put and JSON.parse is not load-bearing.
  const bodyText = await upstream.text();
  const cachedResponse = new Response(bodyText, {
    status: 200,
    headers: {
      "content-type": "application/json",
      "cache-control": "public, max-age=300",
    },
  });
  await cache.put(cacheKey, cachedResponse.clone());
  return JSON.parse(bodyText);
}

function resolveRoute(pathname) {
  if (pathname === "/" || pathname === "") {
    return { type: "install_latest" };
  }

  if (pathname === "/latest") {
    return { type: "api_latest" };
  }

  const match = pathname.match(VERSION_PATTERN);
  if (match) {
    return { type: "versioned", version: match[1] };
  }

  return { type: "not_found" };
}

// Named exports for unit testing — these helpers have no dependency on the
// Cloudflare Workers runtime and are safe to import in Node.js test suites.
export { parseUserAgent, targetTriple, buildLatestPayload };

export default {
  async fetch(request, env) {
    if (request.method !== "GET") {
      return new Response(`Method ${request.method} not allowed.`, {
        status: 405,
        headers: { Allow: "GET" },
      });
    }

    const url = new URL(request.url);
    const route = resolveRoute(url.pathname);
    const ua = request.headers.get("user-agent") || "";

    if (route.type === "not_found") {
      return new Response("Not found", { status: 404 });
    }

    if (route.type === "api_latest") {
      let data;
      try {
        data = await resolveLatest(env);
      } catch (errResponse) {
        // resolveLatest throws a pre-built Response on upstream failure.
        return errResponse;
      }

      const version = data.tag_name.replace(/^creft-v/, "");
      const tag = data.tag_name;
      logEvent(env, "latest", parseUserAgent(ua).version, ua, "/latest");

      return new Response(JSON.stringify(buildLatestPayload(version, tag, ua)), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }

    // Install script routes: / and /v<x.y.z>
    const version = route.type === "versioned" ? route.version : "latest";
    logEvent(env, "install", version, ua, url.pathname);

    let script = INSTALL_SCRIPT;
    let cacheControl = "public, max-age=3600, s-maxage=86400";

    if (route.type === "versioned") {
      script = `CREFT_VERSION='v${route.version}'; export CREFT_VERSION\n` + INSTALL_SCRIPT;
      cacheControl = "public, max-age=31536000, s-maxage=31536000, immutable";
    }

    return new Response(script, {
      status: 200,
      headers: {
        "content-type": "text/plain; charset=utf-8",
        "cache-control": cacheControl,
      },
    });
  },
};
