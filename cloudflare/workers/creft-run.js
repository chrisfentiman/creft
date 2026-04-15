// This file is a template. The deploy skill (creft deploy install-script)
// generates the actual worker by splitting at the marker comment below
// and injecting the contents of scripts/install.sh.
// Do not deploy this file directly — use `creft deploy install-script`.

const INSTALL_SCRIPT = `
// __INSTALL_SCRIPT_CONTENT__
`;

const VERSION_PATTERN = /^\/v?(\d+\.\d+\.\d+)$/;

function resolveRoute(pathname) {
  if (pathname === "/" || pathname === "") {
    return { type: "latest" };
  }

  const match = pathname.match(VERSION_PATTERN);
  if (match) {
    return { type: "versioned", version: `v${match[1]}` };
  }

  return { type: "not_found" };
}

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

    if (route.type === "not_found") {
      return new Response("Not found", { status: 404 });
    }

    // Log every install script delivery to Analytics Engine.
    // Guard with env.CREFT_ANALYTICS check so the worker runs cleanly under `wrangler dev`
    // where the Analytics Engine binding is not available.
    const source = url.searchParams.get("source") || "organic";
    const version = route.type === "versioned" ? route.version : "latest";

    if (env.CREFT_ANALYTICS) {
      env.CREFT_ANALYTICS.writeDataPoint({
        blobs: [source, version, url.pathname],
        doubles: [1],
        indexes: [source],
      });
    }

    let script = INSTALL_SCRIPT;
    let cacheControl = "public, max-age=3600, s-maxage=86400";

    if (route.type === "versioned") {
      script = `CREFT_VERSION='${route.version}'; export CREFT_VERSION\n` + INSTALL_SCRIPT;
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
