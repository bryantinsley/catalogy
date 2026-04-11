// ── Dashboard Page ─────────────────────────────────────────
async function renderDashboard(outlet) {
  outlet.innerHTML = `
    <div class="page-header"><h1>Dashboard</h1><p>Overview of your media catalog</p></div>
    <div id="progress-section" style="display:none;margin-bottom:20px"></div>
    <div class="section"><div class="section-title">Setup Status</div><div id="dash-status" class="card-grid"></div></div>
    <div class="section"><div class="section-title">Quick Stats</div><div id="dash-stats" class="card-grid"></div></div>
    <div class="section quick-actions" id="dash-actions"></div>
    <div id="dash-queue" class="section"></div>`;

  try {
    const [setup, stats] = await Promise.all([api.getSetupStatus(), api.getStatsFull()]);

    document.getElementById('dash-status').innerHTML =
      statusCard('FFmpeg', setup.ffmpeg, setup.ffmpeg ? 'Available' : 'Not found') +
      statusCard('FFprobe', setup.ffprobe, setup.ffprobe ? 'Available' : 'Not found') +
      statusCard('CLIP Models', setup.models, setup.models ? 'Loaded' : 'Not found') +
      statusCard('Database', setup.database, setup.database ? 'Active' : 'Not initialized') +
      statusCard('Catalog', setup.catalog.exists, setup.catalog.exists ? `${setup.catalog.count} items` : 'Empty');

    document.getElementById('dash-stats').innerHTML =
      statCard(stats.total_items, 'Catalog Items') +
      statCard(stats.files_tracked, 'Files Tracked') +
      statCard(stats.queue.pending, 'Queue Pending') +
      statCard(stats.queue.completed, 'Completed');

    document.getElementById('dash-actions').innerHTML = `
      <button class="btn-primary" onclick="promptScan()">Scan Directory</button>
      <button class="btn-secondary" onclick="startIngest()">Run Ingest</button>
      <button class="btn-secondary" onclick="location.hash='#/search'">Search</button>`;

    if (stats.last_scan) {
      document.getElementById('dash-queue').innerHTML = `
        <div class="section-title">Activity</div>
        <div class="card"><span style="color:var(--text-muted)">Last scan:</span> ${escapeHtml(stats.last_scan)}</div>`;
    }

    updateProgressUI();
  } catch (e) {
    outlet.innerHTML = `<div class="empty-state"><h3>Unable to load dashboard</h3><p>${escapeHtml(e.message)}</p></div>`;
  }
}

function promptScan() {
  const path = prompt('Enter directory path to scan:', '');
  if (path) {
    api.scan(path).then(() => {
      alert('Scan started!');
      renderPage('dashboard');
    }).catch(e => alert('Scan failed: ' + e.message));
  }
}

function startIngest() {
  api.ingest(null).then(() => {
    alert('Ingest started!');
    renderPage('dashboard');
  }).catch(e => alert('Ingest failed: ' + e.message));
}

// ── Browse Page ────────────────────────────────────────────
let browseState = { page: 1, type: 'all', sort: 'date' };

async function renderBrowse(outlet) {
  outlet.innerHTML = `
    <div class="page-header"><h1>Browse</h1><p>Browse your indexed media</p></div>
    ${filterBar([
      { type: 'select', id: 'browse-type', value: browseState.type, onChange: 'browseFilterChange()', options: [
        { value: 'all', label: 'All Media' }, { value: 'image', label: 'Images' }, { value: 'video', label: 'Videos' }
      ]},
      { type: 'select', id: 'browse-sort', value: browseState.sort, onChange: 'browseFilterChange()', options: [
        { value: 'date', label: 'Date' }, { value: 'name', label: 'Name' }, { value: 'size', label: 'Size' }
      ]}
    ])}
    <div id="browse-grid"></div>
    <div id="browse-pagination"></div>`;

  await loadBrowseItems();
}

async function loadBrowseItems() {
  const grid = document.getElementById('browse-grid');
  const pag = document.getElementById('browse-pagination');
  if (!grid) return;

  grid.innerHTML = '<div class="thumb-grid">' + Array(8).fill('<div class="skeleton skeleton-thumb"></div>').join('') + '</div>';

  try {
    const data = await api.getBrowse({ page: browseState.page, per_page: 50, type: browseState.type, sort: browseState.sort });
    if (data.items.length === 0) {
      grid.innerHTML = emptyState('\ud83d\uddbc\ufe0f', 'No media indexed yet', 'Scan a directory and run ingest to populate your catalog.',
        '<button class="btn-primary" onclick="promptScan()">Scan Now</button>');
      if (pag) pag.innerHTML = '';
      return;
    }

    window._browseItems = data.items;
    grid.innerHTML = '<div class="thumb-grid">' +
      data.items.map((item, i) => thumbnailItem(item, `openLightbox(window._browseItems, ${i})`)).join('') +
      '</div>';

    if (pag) pag.innerHTML = pagination(data.page, data.total, 50, 'browsePage');
  } catch (e) {
    grid.innerHTML = `<div class="empty-state"><p>Error loading media: ${escapeHtml(e.message)}</p></div>`;
  }
}

function browseFilterChange() {
  browseState.type = document.getElementById('browse-type')?.value || 'all';
  browseState.sort = document.getElementById('browse-sort')?.value || 'date';
  browseState.page = 1;
  loadBrowseItems();
}

function browsePage(p) {
  browseState.page = p;
  loadBrowseItems();
}

// ── Search Page ────────────────────────────────────────────
let searchState = { query: '', results: [] };

async function renderSearch(outlet) {
  outlet.innerHTML = `
    <div class="page-header"><h1>Search</h1><p>Find media using natural language</p></div>
    <div class="search-box">
      <input type="text" id="search-input" placeholder="Describe what you're looking for..." value="${escapeHtml(searchState.query)}"
        onkeydown="if(event.key==='Enter')doSearch()" aria-label="Search query">
    </div>
    <div class="filter-bar">
      <button class="btn-primary" onclick="doSearch()">Search</button>
    </div>
    <div id="search-results"></div>`;

  if (searchState.results.length > 0) {
    renderSearchResults();
  } else {
    document.getElementById('search-results').innerHTML = emptyState(
      '\ud83d\udd0d', 'Search your media',
      'Try queries like "sunset at the beach", "dog playing", or "portrait photo"'
    );
  }

  setTimeout(() => document.getElementById('search-input')?.focus(), 100);
}

async function doSearch() {
  const input = document.getElementById('search-input');
  const resultsEl = document.getElementById('search-results');
  if (!input || !resultsEl) return;

  const query = input.value.trim();
  if (!query) return;
  searchState.query = query;

  resultsEl.innerHTML = '<div class="thumb-grid">' + Array(6).fill('<div class="skeleton skeleton-thumb"></div>').join('') + '</div>';

  try {
    const results = await api.search(query, 50);
    searchState.results = results;
    renderSearchResults();
  } catch (e) {
    resultsEl.innerHTML = `<div class="empty-state"><h3>Search unavailable</h3><p>${escapeHtml(e.message)}</p><p>Make sure CLIP models are loaded.</p></div>`;
  }
}

function renderSearchResults() {
  const resultsEl = document.getElementById('search-results');
  if (!resultsEl) return;

  if (searchState.results.length === 0) {
    resultsEl.innerHTML = emptyState('\ud83e\udd37', 'No results', 'Try a different query or broaden your search.');
    return;
  }

  window._searchItems = searchState.results;
  resultsEl.innerHTML = `
    <div style="margin-bottom:12px;color:var(--text-muted);font-size:0.875rem">${searchState.results.length} result(s)</div>
    <div class="thumb-grid">
      ${searchState.results.map((item, i) => thumbnailItem(item, `openLightbox(window._searchItems, ${i})`)).join('')}
    </div>`;
}

// ── Duplicates Page ────────────────────────────────────────
let dupState = { tier: 'exact' };

async function renderDuplicates(outlet) {
  outlet.innerHTML = `
    <div class="page-header"><h1>Duplicates</h1><p>Find and review duplicate media files</p></div>
    <div class="tabs">
      <button class="tab-btn ${dupState.tier === 'exact' ? 'active' : ''}" onclick="loadDuplicates('exact')">Exact</button>
      <button class="tab-btn ${dupState.tier === 'visual' ? 'active' : ''}" onclick="loadDuplicates('visual')">Visual</button>
      <button class="tab-btn ${dupState.tier === 'cross-video' ? 'active' : ''}" onclick="loadDuplicates('cross-video')">Cross-Video</button>
    </div>
    <div id="dup-results"></div>`;

  await loadDuplicates(dupState.tier);
}

async function loadDuplicates(tier) {
  dupState.tier = tier;
  // Update tab active state
  document.querySelectorAll('.tab-btn').forEach(btn => {
    btn.classList.toggle('active', btn.textContent.toLowerCase().replace('-', '') === tier.replace('-', ''));
  });

  const resultsEl = document.getElementById('dup-results');
  if (!resultsEl) return;

  resultsEl.innerHTML = '<div class="skeleton skeleton-card" style="height:100px;margin:12px 0"></div>'.repeat(3);

  try {
    const data = await api.getDedup({ tier, threshold: 0.92 });
    let groups = [];

    if (data.exact && data.exact.length > 0) {
      groups = data.exact.map(group => ({
        label: `Hash: ${group.file_hash?.substring(0, 12) || 'unknown'}... (${group.files?.length || 0} copies)`,
        items: (group.files || []).map(f => ({
          name: f.file_path?.split('/').pop() || 'unknown',
          path: f.file_path || '',
          size: f.file_size || 0,
          hash: f.file_hash || '',
        })),
      }));
    }

    if (data.visual && data.visual.length > 0) {
      groups = data.visual.map(cluster => ({
        label: `Visual cluster (${cluster.items?.length || 0} items, similarity: ${((cluster.min_similarity || 0) * 100).toFixed(0)}%+)`,
        items: (cluster.items || []).map(item => ({
          id: item.id || '',
          name: item.file_name || '',
          path: item.file_path || '',
          media_type: item.media_type || '',
        })),
        similarity: cluster.min_similarity,
      }));
    }

    if (data.cross_video && data.cross_video.length > 0) {
      groups = data.cross_video.map(dup => ({
        label: `Cross-video match (similarity: ${((dup.similarity || 0) * 100).toFixed(1)}%)`,
        items: [
          { id: dup.frame_id || '', name: dup.frame_file_name || '', path: dup.frame_file_path || '' },
          { id: dup.match_id || '', name: dup.match_file_name || '', path: dup.match_file_path || '' },
        ],
        similarity: dup.similarity,
      }));
    }

    if (groups.length === 0) {
      resultsEl.innerHTML = emptyState('\u2705', 'No duplicates found', 'No duplicate media detected in the current catalog.');
      return;
    }

    resultsEl.innerHTML = groups.map(g => `
      <div class="dup-group">
        <div class="dup-group-header">${escapeHtml(g.label)}${g.similarity ? ` <span class="dup-score">${(g.similarity * 100).toFixed(1)}%</span>` : ''}</div>
        <div class="dup-group-items">
          ${g.items.map(item => `
            <div class="dup-item" ${item.id ? `onclick="openLightbox([${JSON.stringify(item).replace(/"/g, '&quot;')}], 0)"` : ''}>
              ${item.id ? `<img class="thumb-img" src="/api/thumb/${encodeURIComponent(item.id)}" loading="lazy" onerror="this.style.display='none'">` : ''}
              <div class="dup-detail" title="${escapeHtml(item.path)}">${escapeHtml(item.name)}</div>
              ${item.size ? `<div class="dup-detail">${formatSize(item.size)}</div>` : ''}
              <div class="dup-detail" style="font-size:0.6rem">${escapeHtml(item.path)}</div>
            </div>
          `).join('')}
        </div>
      </div>
    `).join('');
  } catch (e) {
    resultsEl.innerHTML = `<div class="empty-state"><h3>Error loading duplicates</h3><p>${escapeHtml(e.message)}</p></div>`;
  }
}

// ── Setup Page ─────────────────────────────────────────────
async function renderSetup(outlet) {
  outlet.innerHTML = `
    <div class="page-header"><h1>Setup</h1><p>System requirements and configuration status</p></div>
    <div id="setup-checklist" class="card"><ul class="checklist"></ul></div>
    <div class="section" style="margin-top:24px">
      <div class="section-title">Getting Started</div>
      <div class="card" style="font-size:0.875rem;line-height:1.8">
        <p><strong>1. Install FFmpeg</strong> (for video processing):</p>
        <code style="color:var(--cyan)">brew install ffmpeg</code> or <code style="color:var(--cyan)">apt install ffmpeg</code>
        <p style="margin-top:12px"><strong>2. Download CLIP models</strong> (for semantic search):</p>
        <p style="color:var(--text-muted)">Place visual.onnx, text.onnx, and tokenizer.json in your model directory.</p>
        <p style="margin-top:12px"><strong>3. Scan your media</strong>:</p>
        <code style="color:var(--cyan)">catalogy scan --path ~/Photos</code>
        <p style="margin-top:12px"><strong>4. Run ingest pipeline</strong>:</p>
        <code style="color:var(--cyan)">catalogy ingest</code>
      </div>
    </div>`;

  try {
    const setup = await api.getSetupStatus();
    const checks = [
      { name: 'FFmpeg', ok: setup.ffmpeg, hint: setup.ffmpeg ? 'Video processing available' : 'Install ffmpeg for video support' },
      { name: 'FFprobe', ok: setup.ffprobe, hint: setup.ffprobe ? 'Media analysis available' : 'Install ffprobe (included with ffmpeg)' },
      { name: 'CLIP Models', ok: setup.models, hint: setup.models ? 'Semantic search ready' : 'Place visual.onnx, text.onnx, tokenizer.json in model dir' },
      { name: 'State Database', ok: setup.database, hint: setup.database ? 'Pipeline state tracked' : 'Run catalogy scan to initialize' },
      { name: 'Catalog', ok: setup.catalog.exists, hint: setup.catalog.exists ? `${setup.catalog.count} items indexed` : 'Run catalogy ingest to populate' },
    ];

    document.querySelector('#setup-checklist .checklist').innerHTML = checks.map(c => `
      <li class="checklist-item">
        <div class="check-icon ${c.ok ? 'pass' : 'fail'}">${c.ok ? '\u2713' : '\u2717'}</div>
        <div class="checklist-info">
          <div class="check-name">${escapeHtml(c.name)}</div>
          <div class="check-hint">${escapeHtml(c.hint)}</div>
        </div>
      </li>
    `).join('');
  } catch (e) {
    document.querySelector('#setup-checklist').innerHTML = `<p>Error loading setup status: ${escapeHtml(e.message)}</p>`;
  }
}
