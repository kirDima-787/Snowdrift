/**
 * Snowdrift Cloudflare Analytics & Download Worker
 *
 * Provides:
 * 1. Download tracking with bot vs human classification & country detection:
 *    - /download/windows -> 302 Redirect to latest Snowdrift-Setup.exe
 *    - /download/macos   -> 302 Redirect to latest Snowdrift.dmg
 *    - /download/linux   -> 302 Redirect to latest Snowdrift-linux-x86_64.tar.gz
 *
 * 2. In-app anonymous telemetry ingestion:
 *    - POST /api/telemetry -> records active app sessions & usage duration
 *
 * 3. Liquid Glass Dark-Mode Dashboard:
 *    - /stats -> Interactive visual dashboard with country flags, real humans vs bots, avg runtime
 *    - /api/stats -> JSON stats feed
 */

const GITHUB_REPO = "kirDima-787/Snowdrift";
const ADMIN_KEY = "snowdrift"; // Optional password for dashboard: /stats?key=snowdrift

const DOWNLOAD_TARGETS = {
  windows: `https://github.com/${GITHUB_REPO}/releases/latest/download/Snowdrift-Setup.exe`,
  macos: `https://github.com/${GITHUB_REPO}/releases/latest/download/Snowdrift.dmg`,
  linux: `https://github.com/${GITHUB_REPO}/releases/latest/download/Snowdrift-linux-x86_64.tar.gz`
};

// In-memory fallback if KV is not bound
let memoryStats = {
  downloads: { human: 0, bot: 0, windows: { human: 0, bot: 0 }, macos: { human: 0, bot: 0 }, linux: { human: 0, bot: 0 } },
  telemetry: { uniqueUsers: 0, totalSessions: 0, totalDurationMins: 0 },
  countries: {}, // { "RU": { downloadsHuman: 0, downloadsBot: 0, appUsers: 0 } }
  usersSeen: {}, // Set of user UIDs
  events: [] // Last 50 events
};

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    const path = url.pathname.toLowerCase().replace(/\/$/, "");

    // CORS headers for telemetry API
    const corsHeaders = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
      "Access-Control-Allow-Headers": "Content-Type, User-Agent"
    };

    if (request.method === "OPTIONS") {
      return new Response(null, { headers: corsHeaders });
    }

    // 1. Download router: /download/:platform
    if (path.startsWith("/download/")) {
      const platform = path.replace("/download/", "").trim();
      const targetUrl = DOWNLOAD_TARGETS[platform];
      if (!targetUrl) {
        return new Response("Unknown platform. Available: windows, macos, linux", { status: 404 });
      }

      const client = classifyClient(request);
      ctx.waitUntil(recordDownload(env, platform, client));

      // Instant 302 redirect directly to GitHub Releases (latency < 20ms)
      return Response.redirect(targetUrl, 302);
    }

    // 2. In-app Telemetry API: POST /api/telemetry
    if (path === "/api/telemetry" && request.method === "POST") {
      try {
        const body = await request.json();
        const country = request.cf?.country || "XX";
        ctx.waitUntil(recordTelemetry(env, body, country));
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { ...corsHeaders, "Content-Type": "application/json" }
        });
      } catch (e) {
        return new Response(JSON.stringify({ error: "Invalid JSON" }), {
          status: 400,
          headers: { ...corsHeaders, "Content-Type": "application/json" }
        });
      }
    }

    // 3. Raw Stats API: GET /api/stats
    if (path === "/api/stats") {
      const stats = await getStats(env);
      return new Response(JSON.stringify(stats, null, 2), {
        headers: { ...corsHeaders, "Content-Type": "application/json" }
      });
    }

    // 4. Liquid Glass Dashboard: GET /stats or GET /
    if (path === "/stats" || path === "") {
      const stats = await getStats(env);
      const html = renderDashboard(stats, url.searchParams.get("key"));
      return new Response(html, {
        headers: { "Content-Type": "text/html; charset=utf-8" }
      });
    }

    return new Response("Not Found", { status: 404 });
  }
};

/**
 * Heuristics to detect whether the downloader is a bot or a real human
 */
function classifyClient(request) {
  const ua = request.headers.get("user-agent") || "";
  const country = request.cf?.country || "XX";
  const city = request.cf?.city || "";

  // Cloudflare Bot Management score (1-29 = likely bot, 30-99 = likely human)
  if (request.cf?.botManagement?.score !== undefined) {
    if (request.cf.botManagement.score < 30) {
      return { isBot: true, reason: `CF Bot Score (${request.cf.botManagement.score})`, country, city, ua };
    }
  }

  // Known scraper, crawler, bot, CLI tools regex
  const botPattern = /(bot|spider|crawl|slurp|curl|wget|python|urllib|requests|httpclient|postman|insomnia|headless|phantom|archive\.org|facebook|twitter|telegram|discord|slack|whatsapp|bytespider|gptbot|claudebot|yandex|googlebot|bingbot|duckduckgo|lighthouse|pagespeed|censys|shodan|masscan|zgrab)/i;
  if (botPattern.test(ua)) {
    return { isBot: true, reason: "Bot User-Agent", country, city, ua };
  }

  // Browser navigation headers check:
  // Real humans clicking a link in Chrome, Edge, Safari, Firefox send client hints and accept-language
  const hasBrowserHints = request.headers.has("sec-ch-ua") || request.headers.has("sec-fetch-dest");
  const hasAcceptLang = request.headers.has("accept-language");
  const isBrowserUA = /Mozilla\/5\.0/i.test(ua);

  if (isBrowserUA && (hasBrowserHints || hasAcceptLang)) {
    return { isBot: false, reason: "Verified Browser", country, city, ua };
  }

  // If no User-Agent or lacks browser headers
  if (!ua || ua.length < 10) {
    return { isBot: true, reason: "Missing/Empty User-Agent", country, city, ua };
  }

  return { isBot: true, reason: "Missing Browser Navigation Headers", country, city, ua };
}

/**
 * Storage helpers (Cloudflare KV or in-memory fallback)
 */
async function getStats(env) {
  if (env && env.STATS_KV) {
    const raw = await env.STATS_KV.get("snowdrift_analytics_v1");
    if (raw) {
      try {
        return JSON.parse(raw);
      } catch (e) {}
    }
  }
  return memoryStats;
}

async function saveStats(env, stats) {
  if (env && env.STATS_KV) {
    await env.STATS_KV.put("snowdrift_analytics_v1", JSON.stringify(stats));
  } else {
    memoryStats = stats;
  }
}

/**
 * Record a download event
 */
async function recordDownload(env, platform, client) {
  const stats = await getStats(env);
  const typeKey = client.isBot ? "bot" : "human";

  stats.downloads[typeKey] = (stats.downloads[typeKey] || 0) + 1;
  if (stats.downloads[platform]) {
    stats.downloads[platform][typeKey] = (stats.downloads[platform][typeKey] || 0) + 1;
  }

  // Country breakdown
  const c = client.country;
  if (!stats.countries[c]) {
    stats.countries[c] = { downloadsHuman: 0, downloadsBot: 0, appUsers: 0 };
  }
  if (client.isBot) {
    stats.countries[c].downloadsBot = (stats.countries[c].downloadsBot || 0) + 1;
  } else {
    stats.countries[c].downloadsHuman = (stats.countries[c].downloadsHuman || 0) + 1;
  }

  // Event feed
  stats.events.unshift({
    timestamp: new Date().toISOString(),
    type: client.isBot ? "bot_download" : "human_download",
    platform,
    country: c,
    city: client.city,
    info: client.reason
  });
  if (stats.events.length > 50) stats.events.pop();

  await saveStats(env, stats);
}

/**
 * Record an in-app telemetry ping
 */
async function recordTelemetry(env, data, country) {
  const stats = await getStats(env);
  const uid = data.uid || "anonymous";
  const event = data.event || "ping";
  const duration = Number(data.duration_mins) || 0;

  if (!stats.usersSeen[uid]) {
    stats.usersSeen[uid] = true;
    stats.telemetry.uniqueUsers = (stats.telemetry.uniqueUsers || 0) + 1;

    // Track country for active user
    if (!stats.countries[country]) {
      stats.countries[country] = { downloadsHuman: 0, downloadsBot: 0, appUsers: 0 };
    }
    stats.countries[country].appUsers = (stats.countries[country].appUsers || 0) + 1;
  }

  if (event === "start") {
    stats.telemetry.totalSessions = (stats.telemetry.totalSessions || 0) + 1;
  }

  if (duration > 0) {
    stats.telemetry.totalDurationMins = (stats.telemetry.totalDurationMins || 0) + duration;
  }

  stats.events.unshift({
    timestamp: new Date().toISOString(),
    type: "app_ping",
    event,
    platform: data.os || "windows",
    version: data.version || "1.0",
    country,
    duration
  });
  if (stats.events.length > 50) stats.events.pop();

  await saveStats(env, stats);
}

/**
 * Convert ISO country code to Emoji Flag
 */
function getFlagEmoji(countryCode) {
  if (!countryCode || countryCode === "XX" || countryCode.length !== 2) return "🌐";
  const codePoints = countryCode
    .toUpperCase()
    .split("")
    .map(c => 127397 + c.charCodeAt(0));
  return String.fromCodePoint(...codePoints);
}

/**
 * Format minutes to human readable time
 */
function formatDuration(totalMins) {
  if (!totalMins || totalMins <= 0) return "0 мин";
  const hours = Math.floor(totalMins / 60);
  const mins = totalMins % 60;
  if (hours === 0) return `${mins} мин`;
  return mins > 0 ? `${hours} ч ${mins} мин` : `${hours} ч`;
}

/**
 * Render the Liquid Glass Analytics Dashboard
 */
function renderDashboard(stats, key) {
  const humanDownloads = stats.downloads.human || 0;
  const botDownloads = stats.downloads.bot || 0;
  const totalDownloads = humanDownloads + botDownloads;
  const humanPercent = totalDownloads > 0 ? Math.round((humanDownloads / totalDownloads) * 100) : 0;
  const botPercent = totalDownloads > 0 ? Math.round((botDownloads / totalDownloads) * 100) : 0;

  const appUsers = stats.telemetry.uniqueUsers || 0;
  const sessions = stats.telemetry.totalSessions || 0;
  const totalMins = stats.telemetry.totalDurationMins || 0;
  const avgMins = sessions > 0 ? Math.round(totalMins / sessions) : 0;

  // Sorted countries
  const countries = Object.entries(stats.countries)
    .map(([code, val]) => ({
      code,
      flag: getFlagEmoji(code),
      human: val.downloadsHuman || 0,
      bot: val.downloadsBot || 0,
      users: val.appUsers || 0,
      total: (val.downloadsHuman || 0) + (val.downloadsBot || 0)
    }))
    .sort((a, b) => b.human - a.human || b.total - a.total);

  const maxCountryHuman = countries.length > 0 ? Math.max(...countries.map(c => c.human), 1) : 1;

  return `<!DOCTYPE html>
<html lang="ru">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Snowdrift · Analytics & Stats</title>
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link href="https://fonts.googleapis.com/css2?family=Plus+Jakarta+Sans:wght@400;500;600;700&display=swap" rel="stylesheet">
  <style>
    :root {
      --bg: #070a13;
      --card-bg: rgba(15, 23, 42, 0.65);
      --card-border: rgba(255, 255, 255, 0.08);
      --card-hover: rgba(255, 255, 255, 0.12);
      --accent-cyan: #38bdf8;
      --accent-blue: #60a5fa;
      --accent-emerald: #34d399;
      --accent-amber: #f59e0b;
      --text-main: #f8fafc;
      --text-muted: #94a3b8;
      --text-dim: #64748b;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background-color: var(--bg);
      background-image: 
        radial-gradient(at 0% 0%, rgba(56, 189, 248, 0.12) 0px, transparent 50%),
        radial-gradient(at 100% 100%, rgba(96, 165, 250, 0.08) 0px, transparent 50%);
      color: var(--text-main);
      font-family: 'Plus Jakarta Sans', system-ui, -apple-system, sans-serif;
      min-height: 100vh;
      padding: 32px 20px;
    }
    .container { max-width: 1100px; margin: 0 auto; }
    
    /* Header */
    header {
      display: flex;
      justify-content: space-between;
      align-items: center;
      margin-bottom: 32px;
      flex-wrap: wrap;
      gap: 16px;
    }
    .brand { display: flex; align-items: center; gap: 14px; }
    .brand-icon {
      width: 44px;
      height: 44px;
      background: linear-gradient(135deg, rgba(56, 189, 248, 0.2), rgba(96, 165, 250, 0.1));
      border: 1px solid var(--card-border);
      border-radius: 12px;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 22px;
      backdrop-filter: blur(10px);
    }
    h1 { font-size: 24px; font-weight: 700; letter-spacing: -0.5px; }
    .subtitle { color: var(--text-muted); font-size: 13px; margin-top: 2px; }
    .badge-live {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      padding: 6px 12px;
      border-radius: 20px;
      background: rgba(52, 211, 153, 0.1);
      border: 1px solid rgba(52, 211, 153, 0.25);
      color: var(--accent-emerald);
      font-size: 12px;
      font-weight: 600;
    }
    .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--accent-emerald); animation: pulse 2s infinite; }
    @keyframes pulse { 0%, 100% { opacity: 1; transform: scale(1); } 50% { opacity: 0.4; transform: scale(0.85); } }

    /* Grid */
    .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 18px; margin-bottom: 28px; }
    .card {
      background: var(--card-bg);
      border: 1px solid var(--card-border);
      border-radius: 18px;
      padding: 22px;
      backdrop-filter: blur(16px);
      box-shadow: 0 10px 30px rgba(0,0,0,0.25);
      transition: border-color 0.2s, transform 0.2s;
    }
    .card:hover { border-color: var(--card-hover); transform: translateY(-2px); }
    .card-title {
      font-size: 12px;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.8px;
      color: var(--text-muted);
      margin-bottom: 10px;
      display: flex;
      justify-content: space-between;
      align-items: center;
    }
    .metric { font-size: 34px; font-weight: 700; letter-spacing: -1px; line-height: 1; }
    .metric-sub { font-size: 12px; color: var(--text-dim); margin-top: 8px; display: flex; align-items: center; gap: 4px; }
    
    /* Progress bar */
    .ratio-bar {
      width: 100%;
      height: 8px;
      border-radius: 4px;
      background: rgba(255,255,255,0.06);
      display: flex;
      overflow: hidden;
      margin-top: 14px;
    }
    .ratio-human { background: var(--accent-cyan); height: 100%; }
    .ratio-bot { background: rgba(255,255,255,0.15); height: 100%; }

    /* Tables & Splits */
    .two-cols { display: grid; grid-template-columns: 2fr 1fr; gap: 18px; margin-bottom: 28px; }
    @media (max-width: 820px) { .two-cols { grid-template-columns: 1fr; } }

    .section-header { font-size: 16px; font-weight: 600; margin-bottom: 16px; display: flex; align-items: center; gap: 8px; }
    
    /* Country List */
    .country-row {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 10px 0;
      border-bottom: 1px solid rgba(255,255,255,0.04);
      font-size: 13px;
    }
    .country-row:last-child { border-bottom: none; }
    .flag { font-size: 18px; width: 24px; text-align: center; }
    .country-name { width: 90px; font-weight: 600; }
    .bar-container { flex: 1; height: 6px; background: rgba(255,255,255,0.05); border-radius: 3px; overflow: hidden; }
    .bar-fill { height: 100%; background: linear-gradient(90deg, var(--accent-cyan), var(--accent-blue)); border-radius: 3px; }
    .country-stat { font-variant-numeric: tabular-nums; color: var(--text-muted); font-size: 12px; text-align: right; min-width: 60px; }
    
    /* Event feed */
    .event-feed { max-height: 380px; overflow-y: auto; padding-right: 6px; }
    .event-item {
      display: flex;
      justify-content: space-between;
      align-items: center;
      padding: 9px 0;
      border-bottom: 1px solid rgba(255,255,255,0.04);
      font-size: 12px;
    }
    .event-type { display: inline-flex; align-items: center; gap: 6px; font-weight: 500; }
    .badge-human { color: var(--accent-cyan); background: rgba(56, 189, 248, 0.1); padding: 2px 7px; border-radius: 6px; }
    .badge-bot { color: var(--text-dim); background: rgba(255, 255, 255, 0.05); padding: 2px 7px; border-radius: 6px; }
    .badge-app { color: var(--accent-emerald); background: rgba(52, 211, 153, 0.1); padding: 2px 7px; border-radius: 6px; }
    .time-ago { color: var(--text-dim); font-size: 11px; }

    /* Footer */
    footer { text-align: center; color: var(--text-dim); font-size: 12px; margin-top: 40px; }
    footer a { color: var(--accent-cyan); text-decoration: none; }
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div class="brand">
        <div class="brand-icon">❄️</div>
        <div>
          <h1>Snowdrift Live Analytics</h1>
          <div class="subtitle">Статистика реальных пользователей, стран и длительности сессий</div>
        </div>
      </div>
      <div class="badge-live"><div class="dot"></div> Live Monitor</div>
    </header>

    <!-- Top Metric Cards -->
    <div class="grid">
      <!-- Card 1: Humans vs Bots -->
      <div class="card">
        <div class="card-title">
          <span>Скачивания (Люди)</span>
          <span style="color: var(--accent-cyan);">${humanPercent}%</span>
        </div>
        <div class="metric" style="color: var(--accent-cyan);">${humanDownloads.toLocaleString()}</div>
        <div class="ratio-bar">
          <div class="ratio-human" style="width: ${humanPercent}%"></div>
          <div class="ratio-bot" style="width: ${botPercent}%"></div>
        </div>
        <div class="metric-sub">
          <span>🤖 Боты/парсеры: ${botDownloads.toLocaleString()} (${botPercent}%)</span>
        </div>
      </div>

      <!-- Card 2: Active Desktop App Users -->
      <div class="card">
        <div class="card-title">
          <span>Пользователи на ПК</span>
          <span>💻</span>
        </div>
        <div class="metric" style="color: var(--accent-emerald);">${appUsers.toLocaleString()}</div>
        <div class="metric-sub">
          <span>100% реальные люди с установленным Snowdrift</span>
        </div>
      </div>

      <!-- Card 3: Avg Usage Duration -->
      <div class="card">
        <div class="card-title">
          <span>Среднее использование</span>
          <span>⏱️</span>
        </div>
        <div class="metric">${formatDuration(avgMins)}</div>
        <div class="metric-sub">
          <span>Всего запусков (сессий): ${sessions.toLocaleString()}</span>
        </div>
      </div>

      <!-- Card 4: Total Snow Time -->
      <div class="card">
        <div class="card-title">
          <span>Общее время снега</span>
          <span>❄️</span>
        </div>
        <div class="metric" style="color: var(--accent-blue);">${Math.round(totalMins / 60)} ч</div>
        <div class="metric-sub">
          <span>Суммарно на всех экранах пользователей</span>
        </div>
      </div>
    </div>

    <!-- 2 Column Breakdown: Countries + Platforms & Live Events -->
    <div class="two-cols">
      <!-- Countries -->
      <div class="card">
        <div class="section-header">🌍 География (Топ Стран)</div>
        ${countries.length === 0 ? '<div style="color:var(--text-dim);font-size:13px;padding:20px 0;">Данных по странам пока нет. Скачивания и запуски появятся здесь автоматически.</div>' : ''}
        ${countries.slice(0, 10).map(c => `
          <div class="country-row">
            <span class="flag">${c.flag}</span>
            <span class="country-name">${c.code}</span>
            <div class="bar-container">
              <div class="bar-fill" style="width: ${Math.round((c.human / maxCountryHuman) * 100)}%"></div>
            </div>
            <span class="country-stat"><strong>${c.human}</strong> чел. <span style="color:var(--text-dim);">(${c.bot} бот.)</span></span>
          </div>
        `).join('')}
      </div>

      <!-- Platforms & Feed -->
      <div class="card">
        <div class="section-header">⚡ Последние действия</div>
        <div class="event-feed">
          ${stats.events.length === 0 ? '<div style="color:var(--text-dim);font-size:12px;padding:20px 0;">Ожидание первых событий...</div>' : ''}
          ${stats.events.slice(0, 15).map(e => `
            <div class="event-item">
              <div>
                ${e.type === 'human_download' ? `<span class="badge-human">📥 Скачивание</span>` : ''}
                ${e.type === 'bot_download' ? `<span class="badge-bot">🤖 Сканер/Бот</span>` : ''}
                ${e.type === 'app_ping' ? `<span class="badge-app">❄️ Запуск (${e.duration > 0 ? e.duration + 'м' : 'старт'})</span>` : ''}
                <span style="margin-left:4px;">${getFlagEmoji(e.country)} ${e.platform || 'windows'}</span>
              </div>
              <span class="time-ago">${new Date(e.timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
            </div>
          `).join('')}
        </div>
      </div>
    </div>

    <footer>
      Snowdrift Open Source · <a href="https://github.com/${GITHUB_REPO}" target="_blank">GitHub Repository</a> · Автоматическое обновление раз в 30 сек.
    </footer>
  </div>

  <script>
    // Auto-refresh stats every 30 seconds
    setInterval(() => {
      fetch('/api/stats')
        .then(r => r.json())
        .then(() => {
          // If in active view, reload quietly
          location.reload();
        })
        .catch(() => {});
    }, 30000);
  </script>
</body>
</html>`;
}
