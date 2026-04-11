// ── API Client ─────────────────────────────────────────────
const api = {
  async get(url) {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`GET ${url}: ${res.status}`);
    return res.json();
  },
  async post(url, body) {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (!res.ok) throw new Error(`POST ${url}: ${res.status}`);
    return res.json();
  },
  getSetupStatus() { return this.get('/api/setup/status'); },
  getStats() { return this.get('/api/stats'); },
  getStatsFull() { return this.get('/api/stats/full'); },
  getFiles(params) { return this.get('/api/files?' + new URLSearchParams(params)); },
  getBrowse(params) { return this.get('/api/browse?' + new URLSearchParams(params)); },
  getDedup(params) { return this.get('/api/dedup?' + new URLSearchParams(params)); },
  search(query, limit) { return this.post('/api/search', { query, limit: limit || 20 }); },
  scan(path) { return this.post('/api/scan', { path }); },
  ingest(stages) { return this.post('/api/ingest', { stages: stages || null }); },
};

// ── Shared State ───────────────────────────────────────────
const state = {
  currentPage: 'dashboard',
  sseSource: null,
  progressData: null,
  lightboxItems: [],
  lightboxIndex: 0,
};

// ── SSE Progress ───────────────────────────────────────────
function connectSSE() {
  if (state.sseSource) state.sseSource.close();
  const es = new EventSource('/api/progress');
  es.onmessage = (e) => {
    try {
      state.progressData = JSON.parse(e.data);
      updateProgressUI();
    } catch (_) {}
  };
  es.onerror = () => {
    es.close();
    state.sseSource = null;
    setTimeout(connectSSE, 5000);
  };
  state.sseSource = es;
}

function updateProgressUI() {
  const el = document.getElementById('progress-section');
  if (!el || !state.progressData) return;
  const d = state.progressData;
  if (d.type === 'idle' || !d.type) {
    el.style.display = 'none';
    return;
  }
  el.style.display = 'block';
  const pct = d.total > 0 ? Math.round((d.processed / d.total) * 100) : 0;
  el.innerHTML = `
    <div class="card" style="border-color: var(--cyan);">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px">
        <strong style="color:var(--cyan)">${capitalize(d.type)}${d.stage ? ' - ' + d.stage : ''}</strong>
        <span style="font-size:0.8rem;color:var(--text-muted)">${d.processed}/${d.total}</span>
      </div>
      <div class="progress-bar"><div class="progress-fill" style="width:${pct}%"></div></div>
      <div class="progress-info"><span>${d.message || ''}</span><span>${pct}%</span></div>
    </div>`;
}

// ── Router ─────────────────────────────────────────────────
const routes = {
  '': 'dashboard',
  'dashboard': 'dashboard',
  'browse': 'browse',
  'search': 'search',
  'duplicates': 'duplicates',
  'setup': 'setup',
};

function navigate(hash) {
  const route = (hash || '').replace('#/', '').replace('#', '');
  const page = routes[route] || 'dashboard';
  state.currentPage = page;

  // Update active nav
  document.querySelectorAll('.sidebar-nav a').forEach(a => {
    a.classList.toggle('active', a.getAttribute('href') === '#/' + page || (page === 'dashboard' && a.getAttribute('href') === '#/'));
  });

  renderPage(page);
}

function renderPage(page) {
  const outlet = document.getElementById('page-outlet');
  if (!outlet) return;

  outlet.innerHTML = '<div class="skeleton skeleton-card" style="height:200px;margin:20px 0"></div>';

  switch (page) {
    case 'dashboard': renderDashboard(outlet); break;
    case 'browse': renderBrowse(outlet); break;
    case 'search': renderSearch(outlet); break;
    case 'duplicates': renderDuplicates(outlet); break;
    case 'setup': renderSetup(outlet); break;
    default: outlet.innerHTML = '<div class="empty-state"><h3>Page not found</h3></div>';
  }
}

// ── Init ───────────────────────────────────────────────────
window.addEventListener('hashchange', () => navigate(location.hash));
window.addEventListener('DOMContentLoaded', () => {
  navigate(location.hash);
  connectSSE();

  // Mobile hamburger
  const hamburger = document.querySelector('.hamburger');
  const sidebar = document.querySelector('.sidebar');
  if (hamburger && sidebar) {
    hamburger.addEventListener('click', () => sidebar.classList.toggle('open'));
    document.addEventListener('click', (e) => {
      if (sidebar.classList.contains('open') && !sidebar.contains(e.target) && e.target !== hamburger) {
        sidebar.classList.remove('open');
      }
    });
  }
});

// ── Helpers ────────────────────────────────────────────────
function capitalize(s) { return s ? s.charAt(0).toUpperCase() + s.slice(1) : ''; }
function formatSize(bytes) {
  if (bytes < 1024) return bytes + ' B';
  if (bytes < 1048576) return (bytes / 1024).toFixed(1) + ' KB';
  if (bytes < 1073741824) return (bytes / 1048576).toFixed(1) + ' MB';
  return (bytes / 1073741824).toFixed(1) + ' GB';
}
function escapeHtml(s) {
  const d = document.createElement('div');
  d.textContent = s;
  return d.innerHTML;
}
