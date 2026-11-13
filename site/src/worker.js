// wholeseek.utilitygods.com: static assets, plus /changelog rendered from GitHub Releases.
// The release notes live on GitHub (written from CHANGELOG.md at release time); this page
// only restyles them, so a new release shows up here without a site deploy.

const REPO = "Utility-Gods/wholeseek";
const API = `https://api.github.com/repos/${REPO}/releases?per_page=30`;
// The API's anonymous limit is per IP, and Cloudflare's outbound IPs are shared, so it often
// answers 403. The releases Atom feed isn't API-limited; its dates are last-edit times.
const FEED = `https://github.com/${REPO}/releases.atom`;
const FRESH_SECONDS = 600; // Re-ask GitHub at most every 10 minutes.
const BACKUP_SECONDS = 30 * 24 * 3600; // Last good page, served if GitHub can't be reached.

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    if (url.pathname === "/changelog" || url.pathname === "/changelog/") {
      return changelog(request, ctx);
    }
    return env.ASSETS.fetch(request);
  },
};

async function fromApi() {
  const res = await fetch(API, {
    headers: {
      Accept: "application/vnd.github.html+json",
      "User-Agent": "wholeseek-site",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (!res.ok) return null;
  const list = await res.json();
  return Array.isArray(list) ? list.filter((r) => !r.draft) : null;
}

function unescapeXml(s) {
  return s
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&");
}

async function fromFeed() {
  const res = await fetch(FEED, { headers: { "User-Agent": "wholeseek-site" } });
  if (!res.ok) return null;
  const xml = await res.text();
  const entries = [...xml.matchAll(/<entry>([\s\S]*?)<\/entry>/g)].map((m) => m[1]);
  const list = entries
    .map((e) => {
      const tag = (e.match(/<id>[^<]*\/([^/<]+)<\/id>/) || [])[1];
      const href = (e.match(/<link[^>]*href="([^"]+)"/) || [])[1];
      const updated = (e.match(/<updated>([^<]+)<\/updated>/) || [])[1];
      const content = (e.match(/<content type="html">([\s\S]*?)<\/content>/) || [])[1] || "";
      return tag && href && updated
        ? { tag_name: unescapeXml(tag), html_url: unescapeXml(href), published_at: updated, body_html: unescapeXml(content) }
        : null;
    })
    .filter(Boolean);
  return entries.length === list.length ? list : null;
}

async function releases() {
  for (const source of [fromApi, fromFeed]) {
    try {
      const list = await source();
      if (list) return list;
    } catch (_) {
      // Try the next source.
    }
  }
  return null;
}

async function changelog(request, ctx) {
  const cache = caches.default;
  const key = new Request(new URL("/changelog", request.url).toString());
  const backupKey = new Request(new URL("/changelog?backup", request.url).toString());
  const hit = await cache.match(key);
  if (hit) return hit;

  const list = await releases();
  if (!list) {
    const backup = await cache.match(backupKey);
    if (backup) return new Response(backup.body, { status: 200, headers: withCache(backup.headers, 60) });
  }

  const ok = list !== null;
  const html = page(list);
  const response = new Response(html, {
    status: ok ? 200 : 503,
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      // Short browser cache; the edge copy below is what keeps GitHub calls rare.
      "Cache-Control": ok ? `public, max-age=${FRESH_SECONDS}` : "no-store",
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "strict-origin-when-cross-origin",
      // GitHub's HTML is already sanitized; no scripts, and images only from GitHub.
      "Content-Security-Policy":
        "default-src 'self'; img-src 'self' https://*.githubusercontent.com https://github.com; style-src 'unsafe-inline'; script-src 'none'; frame-ancestors 'none'",
    },
  });
  if (ok) {
    ctx.waitUntil(cache.put(key, response.clone()));
    const backup = response.clone();
    ctx.waitUntil(cache.put(backupKey, new Response(backup.body, { headers: withCache(backup.headers, BACKUP_SECONDS) })));
  }
  return response;
}

function withCache(headers, seconds) {
  const h = new Headers(headers);
  h.set("Cache-Control", `public, max-age=${seconds}`);
  return h;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function day(iso) {
  const d = new Date(iso);
  return d.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });
}

function entry(r, latest) {
  const version = r.tag_name.replace(/^v/, "");
  const body = (r.body_html || "").trim() || "<p>Bug fixes and improvements.</p>";
  return `<article id="${esc(r.tag_name)}">
  <header><h2><a href="#${esc(r.tag_name)}">${esc(version)}</a>${latest ? '<span class="latest">Latest</span>' : ""}</h2>
  <time datetime="${esc(r.published_at)}">${esc(day(r.published_at))}</time></header>
  <div class="notes">${body}</div>
  <a class="more" href="${esc(r.html_url)}">Downloads and details on GitHub</a>
</article>`;
}

function page(releases) {
  const list =
    releases === null
      ? `<p class="empty">The changelog couldn't be loaded right now. See <a href="https://github.com/${REPO}/releases">releases on GitHub</a>.</p>`
      : releases.length === 0
        ? `<p class="empty">No releases yet.</p>`
        : releases.map((r, i) => entry(r, i === 0)).join("\n");
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Changelog · wholeseek</title>
<meta name="description" content="What's new in each wholeseek release.">
<meta name="theme-color" content="#0a0e1a">
<link rel="icon" href="/icon.svg" type="image/svg+xml">
<style>
*{box-sizing:border-box}
html{background:#0a0e1a;color:#e2e8f0;font:16px/1.6 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;-webkit-font-smoothing:antialiased}
body{margin:0}
a{color:#e2e8f0}
nav{max-width:720px;margin:0 auto;padding:28px 24px 0;display:flex;align-items:center;gap:10px}
nav a{display:flex;align-items:center;gap:10px;text-decoration:none;font-weight:650;font-size:17px}
nav img{width:28px;height:28px}
nav .dl{margin-left:auto;font-weight:500;font-size:14px;color:#94a3b8;transition:color 80ms}
nav .dl:hover{color:#e2e8f0}
main{max-width:720px;margin:0 auto;padding:40px 24px 24px}
h1{font-size:30px;font-weight:650;line-height:1.15;letter-spacing:-.01em;margin:0 0 6px}
.sub{color:#94a3b8;margin:0 0 28px}
article{border-top:1px solid #182033;padding:28px 0}
article header{display:flex;align-items:baseline;gap:12px;flex-wrap:wrap}
article h2{font-size:21px;font-weight:650;margin:0;display:flex;align-items:center;gap:10px}
article h2 a{text-decoration:none}
.latest{font-size:12px;font-weight:600;color:#fb923c;border:1px solid #fb923c;border-radius:999px;padding:1px 8px}
time{color:#94a3b8;font-size:14px}
.notes{margin-top:10px;color:#cbd5e1}
.notes h1,.notes h2,.notes h3{font-size:15px;font-weight:650;color:#e2e8f0;margin:18px 0 6px}
.notes ul{padding-left:20px;margin:6px 0}
.notes li{margin:3px 0}
.notes p{margin:8px 0}
.notes a{color:#e2e8f0;text-decoration:none;border-bottom:1px solid #2a3447}
.notes a:hover{border-color:#94a3b8}
.notes code,.notes tt{font:13.5px ui-monospace,SFMono-Regular,Menlo,monospace;background:#0e1424;border:1px solid #182033;border-radius:4px;padding:1px 5px}
.notes img{max-width:100%;height:auto;border-radius:6px}
.more{display:inline-block;margin-top:10px;font-size:14px;color:#94a3b8;text-decoration:none;transition:color 80ms}
.more:hover{color:#e2e8f0}
.empty{color:#94a3b8}
.empty a{color:#fb923c}
a:focus-visible{outline:2px solid #fb923c;outline-offset:3px;border-radius:2px}
footer{max-width:720px;margin:0 auto;padding:20px 24px 32px;color:#94a3b8;font-size:12.5px;text-align:center}
footer a{color:#94a3b8;text-decoration:none;border-bottom:1px solid #2a3447}
footer a:hover{color:#e2e8f0;border-color:#94a3b8}
@media (max-width:600px){main{padding:32px 16px 16px}nav{padding:20px 16px 0}h1{font-size:26px}}
</style>
</head>
<body>
<nav><a href="/"><img src="/icon.svg" width="28" height="28" alt="">wholeseek</a><a class="dl" href="/">Download</a></nav>
<main>
<h1>Changelog</h1>
<p class="sub">What's new in each release.</p>
${list}
</main>
<footer>Made by <a href="https://utilitygods.com">Utility Gods</a> · Not affiliated with Soulseek.</footer>
</body>
</html>`;
}
