//! Embedded WebChat UI served as static HTML.
//!
//! The production dashboard is assembled at compile time from separate
//! HTML/CSS/JS files under `static/` using `include_str!()`. This keeps
//! single-binary deployment while allowing organized source files.
//!
//! Features:
//! - Alpine.js SPA with hash-based routing (10 panels)
//! - Dark/light theme toggle with system preference detection
//! - Responsive layout with collapsible sidebar
//! - Markdown rendering + syntax highlighting (bundled locally)
//! - WebSocket real-time chat with HTTP fallback
//! - Agent management, workflows, memory browser, audit log, and more

use axum::http::header;
use axum::response::IntoResponse;

/// Compile-time ETag based on the crate version.
const ETAG: &str = concat!("\"openfang-", env!("CARGO_PKG_VERSION"), "\"");

/// Embedded logo PNG for single-binary deployment.
const LOGO_PNG: &[u8] = include_bytes!("../static/logo.png");

/// Embedded favicon ICO for browser tabs.
const FAVICON_ICO: &[u8] = include_bytes!("../static/favicon.ico");

/// GET /logo.png — Serve the OpenFang logo.
pub async fn logo_png() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
        ],
        LOGO_PNG,
    )
}

/// GET /favicon.ico — Serve the OpenFang favicon.
pub async fn favicon_ico() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/x-icon"),
            (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
        ],
        FAVICON_ICO,
    )
}

/// Embedded PWA manifest for installable web app support.
const MANIFEST_JSON: &str = include_str!("../static/manifest.json");

/// Embedded service worker for PWA support.
const SW_JS: &str = include_str!("../static/sw.js");

/// GET /manifest.json — Serve the PWA web app manifest.
pub async fn manifest_json() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/manifest+json"),
            (header::CACHE_CONTROL, "public, max-age=86400, immutable"),
        ],
        MANIFEST_JSON,
    )
}

/// GET /sw.js — Serve the PWA service worker.
pub async fn sw_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        SW_JS,
    )
}

/// GET / — Serve the OpenFang Dashboard single-page application.
///
/// Returns the full SPA with ETag header based on package version for caching.
pub async fn webchat_page() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::ETAG, ETAG),
            (
                header::CACHE_CONTROL,
                "public, max-age=3600, must-revalidate",
            ),
        ],
        WEBCHAT_HTML,
    )
}

/// GET /trading — Serve the Trading Dashboard page.
pub async fn trading_page() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        TRADING_DASHBOARD_HTML,
    )
}

/// The embedded HTML/CSS/JS for the OpenFang Dashboard.
///
/// Assembled at compile time from organized static files.
/// All vendor libraries (Alpine.js, marked.js, highlight.js) are bundled
/// locally — no CDN dependency. Alpine.js is included LAST because it
/// immediately processes x-data directives and fires alpine:init on load.
const WEBCHAT_HTML: &str = concat!(
    include_str!("../static/index_head.html"),
    "<style>\n",
    include_str!("../static/css/theme.css"),
    "\n",
    include_str!("../static/css/layout.css"),
    "\n",
    include_str!("../static/css/components.css"),
    "\n",
    include_str!("../static/vendor/github-dark.min.css"),
    "\n</style>\n",
    include_str!("../static/index_body.html"),
    // Vendor libs: marked + highlight first (used by app.js), then Chart.js
    "<script>\n",
    include_str!("../static/vendor/marked.min.js"),
    "\n</script>\n",
    "<script>\n",
    include_str!("../static/vendor/highlight.min.js"),
    "\n</script>\n",
    "<script>\n",
    include_str!("../static/vendor/chart.umd.min.js"),
    "\n</script>\n",
    // App code
    "<script>\n",
    include_str!("../static/js/api.js"),
    "\n",
    include_str!("../static/js/app.js"),
    "\n",
    include_str!("../static/js/pages/overview.js"),
    "\n",
    include_str!("../static/js/katex.js"),
    "\n",
    include_str!("../static/js/pages/chat.js"),
    "\n",
    include_str!("../static/js/pages/agents.js"),
    "\n",
    include_str!("../static/js/pages/workflows.js"),
    "\n",
    include_str!("../static/js/pages/workflow-builder.js"),
    "\n",
    include_str!("../static/js/pages/channels.js"),
    "\n",
    include_str!("../static/js/pages/skills.js"),
    "\n",
    include_str!("../static/js/pages/hands.js"),
    "\n",
    include_str!("../static/js/pages/scheduler.js"),
    "\n",
    include_str!("../static/js/pages/settings.js"),
    "\n",
    include_str!("../static/js/pages/usage.js"),
    "\n",
    include_str!("../static/js/pages/sessions.js"),
    "\n",
    include_str!("../static/js/pages/logs.js"),
    "\n",
    include_str!("../static/js/pages/wizard.js"),
    "\n",
    include_str!("../static/js/pages/approvals.js"),
    "\n",
    include_str!("../static/js/pages/comms.js"),
    "\n",
    include_str!("../static/js/pages/runtime.js"),
    "\n</script>\n",
    // Alpine.js MUST be last — it processes x-data and fires alpine:init
    "<script>\n",
    include_str!("../static/vendor/alpine.min.js"),
    "\n</script>\n",
    "</body></html>"
);

/// Trading Dashboard HTML — single-page dashboard with inline CSS/JS.
const TRADING_DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Trading Dashboard - OpenFang</title>
    <script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.0/dist/chart.umd.min.js"></script>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0a0e27; color: #e0e0e0; padding: 20px; }
        .container { max-width: 1400px; margin: 0 auto; }
        h1 { color: #00d4ff; margin-bottom: 30px; font-size: 2em; }
        .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 20px; margin-bottom: 30px; }
        .card { background: #1a1f3a; border-radius: 12px; padding: 20px; box-shadow: 0 4px 6px rgba(0,0,0,0.3); }
        .card h2 { color: #00d4ff; font-size: 1.2em; margin-bottom: 15px; border-bottom: 2px solid #00d4ff; padding-bottom: 10px; }
        .stat { display: flex; justify-content: space-between; margin: 10px 0; padding: 8px 0; border-bottom: 1px solid #2a2f4a; }
        .stat:last-child { border-bottom: none; }
        .stat-label { color: #8892b0; }
        .stat-value { font-weight: bold; color: #e0e0e0; }
        .positive { color: #00ff88; }
        .negative { color: #ff4444; }
        table { width: 100%; border-collapse: collapse; margin-top: 10px; }
        th { background: #2a2f4a; padding: 12px; text-align: left; color: #00d4ff; font-weight: 600; }
        td { padding: 10px 12px; border-bottom: 1px solid #2a2f4a; }
        tr:hover { background: #252a45; }
        .badge { display: inline-block; padding: 4px 12px; border-radius: 12px; font-size: 0.85em; font-weight: 600; }
        .badge-open { background: #ffa500; color: #000; }
        .badge-correct { background: #00ff88; color: #000; }
        .badge-incorrect { background: #ff4444; color: #fff; }
        .badge-buy { background: #00d4ff; color: #000; }
        .badge-sell { background: #ff6b6b; color: #fff; }
        .refresh-info { text-align: right; color: #8892b0; font-size: 0.9em; margin-bottom: 10px; }
        .gauge { position: relative; width: 200px; height: 200px; margin: 20px auto; }
        .gauge-circle { fill: none; stroke: #2a2f4a; stroke-width: 20; }
        .gauge-fill { fill: none; stroke: #00ff88; stroke-width: 20; stroke-linecap: round; transition: stroke-dashoffset 0.5s; }
        .gauge-text { position: absolute; top: 50%; left: 50%; transform: translate(-50%, -50%); font-size: 2.5em; font-weight: bold; color: #00ff88; }
    </style>
</head>
<body>
    <div class="container">
        <h1>📈 Trading Dashboard</h1>
        <div class="refresh-info">Auto-refresh: <span id="countdown">60</span>s</div>

        <div class="grid">
            <div class="card">
                <h2>Portfolio Summary</h2>
                <div id="portfolio-summary">Loading...</div>
            </div>
            <div class="card">
                <h2>Prediction Accuracy</h2>
                <div id="accuracy-gauge"></div>
            </div>
            <div class="card">
                <h2>P&L Chart</h2>
                <canvas id="pnl-chart"></canvas>
            </div>
        </div>

        <div class="grid">
            <div class="card">
                <h2>Recent Trades</h2>
                <div id="recent-trades">Loading...</div>
            </div>
            <div class="card">
                <h2>Open Predictions</h2>
                <div id="open-predictions">Loading...</div>
            </div>
        </div>

        <div class="card">
            <h2>Latest Insights</h2>
            <div id="learnings">Loading...</div>
        </div>
    </div>

    <script>
        let countdown = 60;
        let pnlChart = null;

        async function fetchDashboard() {
            try {
                const res = await fetch('/api/trading/dashboard');
                const data = await res.json();

                // Portfolio Summary
                if (data.portfolio) {
                    const p = data.portfolio;
                    const pnlClass = p.pnl_pct >= 0 ? 'positive' : 'negative';
                    document.getElementById('portfolio-summary').innerHTML = `
                        <div class="stat"><span class="stat-label">Cash</span><span class="stat-value">$${p.balance.toFixed(2)}</span></div>
                        <div class="stat"><span class="stat-label">Total Value</span><span class="stat-value">$${p.total_value.toFixed(2)}</span></div>
                        <div class="stat"><span class="stat-label">P&L</span><span class="stat-value ${pnlClass}">${p.pnl_pct >= 0 ? '+' : ''}${p.pnl_pct.toFixed(2)}%</span></div>
                        <div class="stat"><span class="stat-label">Positions</span><span class="stat-value">${p.positions.length}</span></div>
                    `;
                }

                // Accuracy Gauge
                if (data.accuracy_30d) {
                    const acc = data.accuracy_30d.accuracy_pct || 0;
                    const radius = 80;
                    const circumference = 2 * Math.PI * radius;
                    const offset = circumference - (acc / 100) * circumference;
                    document.getElementById('accuracy-gauge').innerHTML = `
                        <svg class="gauge" viewBox="0 0 200 200">
                            <circle class="gauge-circle" cx="100" cy="100" r="${radius}"></circle>
                            <circle class="gauge-fill" cx="100" cy="100" r="${radius}"
                                    stroke-dasharray="${circumference}"
                                    stroke-dashoffset="${offset}"
                                    transform="rotate(-90 100 100)"></circle>
                        </svg>
                        <div class="gauge-text">${acc.toFixed(0)}%</div>
                        <div style="text-align:center; color:#8892b0; margin-top:10px;">30-day accuracy</div>
                    `;
                }

                // Recent Trades
                if (data.recent_trades && data.recent_trades.length > 0) {
                    const rows = data.recent_trades.slice(0, 5).map(t => `
                        <tr>
                            <td>${t.ticker}</td>
                            <td><span class="badge badge-${t.side}">${t.side.toUpperCase()}</span></td>
                            <td>${t.shares}</td>
                            <td>$${t.price.toFixed(2)}</td>
                            <td>${new Date(t.timestamp).toLocaleString()}</td>
                        </tr>
                    `).join('');
                    document.getElementById('recent-trades').innerHTML = `
                        <table><thead><tr><th>Ticker</th><th>Side</th><th>Shares</th><th>Price</th><th>Time</th></tr></thead><tbody>${rows}</tbody></table>
                    `;
                } else {
                    document.getElementById('recent-trades').innerHTML = '<p style="color:#8892b0;">No trades yet</p>';
                }

                // Open Predictions
                if (data.predictions && data.predictions.length > 0) {
                    const rows = data.predictions.slice(0, 5).map(p => `
                        <tr>
                            <td>${p.ticker}</td>
                            <td>${p.direction.toUpperCase()}</td>
                            <td>$${p.target_price.toFixed(2)}</td>
                            <td>${p.confidence.toFixed(0)}%</td>
                            <td><span class="badge badge-${p.status}">${p.status.toUpperCase()}</span></td>
                        </tr>
                    `).join('');
                    document.getElementById('open-predictions').innerHTML = `
                        <table><thead><tr><th>Ticker</th><th>Direction</th><th>Target</th><th>Confidence</th><th>Status</th></tr></thead><tbody>${rows}</tbody></table>
                    `;
                } else {
                    document.getElementById('open-predictions').innerHTML = '<p style="color:#8892b0;">No open predictions</p>';
                }

                // Learnings
                if (data.learnings && data.learnings.length > 0) {
                    const items = data.learnings.map(l => `
                        <div class="stat">
                            <span class="stat-label">${l.category}</span>
                            <span class="stat-value">${l.insight}</span>
                        </div>
                    `).join('');
                    document.getElementById('learnings').innerHTML = items;
                } else {
                    document.getElementById('learnings').innerHTML = '<p style="color:#8892b0;">No insights yet</p>';
                }

                // P&L Chart (simple mock for now)
                if (pnlChart) pnlChart.destroy();
                const ctx = document.getElementById('pnl-chart').getContext('2d');
                pnlChart = new Chart(ctx, {
                    type: 'line',
                    data: {
                        labels: ['Day 1', 'Day 2', 'Day 3', 'Day 4', 'Day 5', 'Day 6', 'Day 7'],
                        datasets: [{
                            label: 'Portfolio Value',
                            data: [100000, 100500, 99800, 101200, 102000, 101500, data.portfolio?.total_value || 100000],
                            borderColor: '#00d4ff',
                            backgroundColor: 'rgba(0, 212, 255, 0.1)',
                            tension: 0.4,
                            fill: true
                        }]
                    },
                    options: {
                        responsive: true,
                        maintainAspectRatio: false,
                        plugins: { legend: { display: false } },
                        scales: {
                            y: { ticks: { color: '#8892b0' }, grid: { color: '#2a2f4a' } },
                            x: { ticks: { color: '#8892b0' }, grid: { color: '#2a2f4a' } }
                        }
                    }
                });

            } catch (err) {
                console.error('Failed to fetch dashboard:', err);
            }
        }

        // Auto-refresh
        setInterval(() => {
            countdown--;
            document.getElementById('countdown').textContent = countdown;
            if (countdown <= 0) {
                countdown = 60;
                fetchDashboard();
            }
        }, 1000);

        // Initial load
        fetchDashboard();
    </script>
</body>
</html>
"#;
