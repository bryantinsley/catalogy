// ── Reusable UI Components ─────────────────────────────────

function statusCard(title, ok, detail) {
  const cls = ok ? 'ok' : 'err';
  const icon = ok ? '\u2713' : '\u2717';
  return `
    <div class="card status-card">
      <div class="status-icon ${cls}">${icon}</div>
      <div class="status-info">
        <h3>${escapeHtml(title)}</h3>
        <p>${escapeHtml(detail)}</p>
      </div>
    </div>`;
}

function statCard(value, label) {
  return `
    <div class="card stat-card">
      <div class="stat-value">${value}</div>
      <div class="stat-label">${escapeHtml(label)}</div>
    </div>`;
}

function thumbnailItem(item, onclick) {
  const src = item.id ? `/api/thumb/${encodeURIComponent(item.id)}` : '';
  const scoreHtml = item.score != null ? `<div class="thumb-score">${(item.score * 100).toFixed(1)}%</div>` : '';
  const meta = item.media_type || '';
  const dims = (item.width && item.height) ? `${item.width}x${item.height}` : '';
  const metaText = [meta, dims].filter(Boolean).join(' \u00b7 ');

  return `
    <div class="thumb-item" data-id="${escapeHtml(item.id || '')}" onclick="${onclick}">
      ${src ? `<img class="thumb-img" src="${src}" alt="${escapeHtml(item.file_name || '')}" loading="lazy" onerror="this.style.display='none'">` : '<div class="thumb-img skeleton skeleton-thumb"></div>'}
      <div class="thumb-info">
        <div class="thumb-name">${escapeHtml(item.file_name || item.name || '')}</div>
        <div class="thumb-meta">${escapeHtml(metaText)}</div>
        ${scoreHtml}
      </div>
    </div>`;
}

function pagination(page, total, perPage, onPageChange) {
  const totalPages = Math.max(1, Math.ceil(total / perPage));
  if (totalPages <= 1) return '';
  const prevDisabled = page <= 1 ? 'disabled' : '';
  const nextDisabled = page >= totalPages ? 'disabled' : '';

  return `
    <div class="pagination">
      <button class="btn-ghost" ${prevDisabled} onclick="${onPageChange}(${page - 1})">Prev</button>
      <span class="page-info">Page ${page} of ${totalPages} (${total} items)</span>
      <button class="btn-ghost" ${nextDisabled} onclick="${onPageChange}(${page + 1})">Next</button>
    </div>`;
}

function emptyState(icon, title, message, actionHtml) {
  return `
    <div class="empty-state">
      <div class="empty-icon">${icon}</div>
      <h3>${escapeHtml(title)}</h3>
      <p>${message}</p>
      ${actionHtml || ''}
    </div>`;
}

function filterBar(options) {
  let html = '<div class="filter-bar">';
  for (const opt of options) {
    if (opt.type === 'select') {
      html += `<select id="${opt.id}" onchange="${opt.onChange}">`;
      for (const o of opt.options) {
        const sel = o.value === opt.value ? 'selected' : '';
        html += `<option value="${o.value}" ${sel}>${escapeHtml(o.label)}</option>`;
      }
      html += '</select>';
    }
  }
  html += '</div>';
  return html;
}

// ── Lightbox ───────────────────────────────────────────────
function openLightbox(items, index) {
  state.lightboxItems = items;
  state.lightboxIndex = index;
  renderLightbox();
}

function closeLightbox() {
  state.lightboxItems = [];
  const lb = document.getElementById('lightbox');
  if (lb) lb.classList.remove('active');
}

function lightboxPrev() {
  if (state.lightboxIndex > 0) {
    state.lightboxIndex--;
    renderLightbox();
  }
}

function lightboxNext() {
  if (state.lightboxIndex < state.lightboxItems.length - 1) {
    state.lightboxIndex++;
    renderLightbox();
  }
}

function renderLightbox() {
  const lb = document.getElementById('lightbox');
  if (!lb || !state.lightboxItems.length) return;

  const item = state.lightboxItems[state.lightboxIndex];
  const isVideo = item.media_type === 'video' || item.media_type === 'video_frame';
  const mediaSrc = `/api/media/${encodeURIComponent(item.id)}`;

  let mediaHtml;
  if (isVideo) {
    mediaHtml = `<video class="lightbox-media" controls autoplay><source src="${mediaSrc}"></video>`;
  } else {
    mediaHtml = `<img class="lightbox-media" src="${mediaSrc}" alt="${escapeHtml(item.file_name || '')}">`;
  }

  const dims = (item.width && item.height) ? `${item.width} x ${item.height}` : 'N/A';
  const duration = item.duration_ms ? `${(item.duration_ms / 1000).toFixed(1)}s` : 'N/A';
  const score = item.score != null ? `${(item.score * 100).toFixed(1)}%` : 'N/A';

  lb.innerHTML = `
    <button class="lightbox-close" onclick="closeLightbox()">\u00d7</button>
    ${state.lightboxItems.length > 1 ? `
      <button class="lightbox-nav prev" onclick="lightboxPrev()">\u2039</button>
      <button class="lightbox-nav next" onclick="lightboxNext()">\u203a</button>
    ` : ''}
    <div class="lightbox-content">
      ${mediaHtml}
      <div class="lightbox-details">
        <h3>${escapeHtml(item.file_name || '')}</h3>
        <div class="detail-row"><span class="label">Type</span><span class="value">${escapeHtml(item.media_type || '')}</span></div>
        <div class="detail-row"><span class="label">Dimensions</span><span class="value">${dims}</span></div>
        <div class="detail-row"><span class="label">Duration</span><span class="value">${duration}</span></div>
        <div class="detail-row"><span class="label">Score</span><span class="value">${score}</span></div>
        <div class="detail-row"><span class="label">Path</span><span class="value" style="word-break:break-all;font-size:0.7rem">${escapeHtml(item.file_path || '')}</span></div>
      </div>
    </div>`;

  lb.classList.add('active');
}

// Global keyboard handler for lightbox
document.addEventListener('keydown', (e) => {
  if (!state.lightboxItems.length) return;
  if (e.key === 'Escape') closeLightbox();
  if (e.key === 'ArrowLeft') lightboxPrev();
  if (e.key === 'ArrowRight') lightboxNext();
});
