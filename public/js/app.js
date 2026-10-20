import { api } from './api.js';
import { icon, hydrateIcons } from './icons.js';
import { BUILTIN_THEMES, FONTS, THEME_VARS, WIDTHS, applyTheme, resolveTheme } from './themes.js';

// ---------- state ----------

const DEFAULT_PREFS = {
  theme: 'auto',
  font: 'serif',
  fontSize: 19,
  lineHeight: 1.7,
  width: 'medium',
  density: 'comfortable',
  showImages: true,
  markReadOnOpen: true,
  filter: 'unread',
};

const state = {
  prefs: { ...DEFAULT_PREFS },
  customThemes: [],
  info: {},
  feeds: [],
  categories: [],
  stats: {},
  view: { type: 'all' },
  search: '',
  articles: [],
  total: 0,
  loading: false,
  requestId: 0,
  selectedId: null,
  current: null,
  collapsed: new Set(),
};

const $ = (sel, root = document) => root.querySelector(sel);

// ---------- DOM helpers ----------

/** Creates an element. Strings become text nodes, so content is never parsed as HTML. */
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'dataset') Object.assign(el.dataset, v);
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (v === true) el.setAttribute(k, '');
    else el.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

/** Like `el.replaceChildren`, but skips null/false (which the DOM would print as text). */
function put(el, ...children) {
  el.replaceChildren(...children.flat().filter((c) => c !== null && c !== undefined && c !== false));
}

function toast(message, type = 'info') {
  const el = h('div', { class: `toast ${type}` }, message);
  $('#toasts').append(el);
  setTimeout(() => {
    el.classList.add('leaving');
    setTimeout(() => el.remove(), 250);
  }, type === 'error' ? 5000 : 2600);
}

const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

function timeAgo(iso) {
  const d = new Date(iso);
  const s = (Date.now() - d) / 1000;
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 7 * 86400) return `${Math.floor(s / 86400)}d`;
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: sameYear ? undefined : 'numeric' });
}

const fullDate = (iso) => new Date(iso).toLocaleString(undefined, { dateStyle: 'long', timeStyle: 'short' });

function hostname(url) {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return '';
  }
}

function hue(str) {
  let x = 0;
  for (const c of str) x = (x * 31 + c.codePointAt(0)) % 360;
  return x;
}

/** A favicon with a colored-letter fallback. */
function feedIcon(feed, size = 18) {
  const title = feed.title || feed.feed_title || '?';
  const letter = (title.match(/[\p{L}\p{N}]/u) || ['?'])[0].toUpperCase();
  const box = h('span', { class: 'feed-icon', style: `background:hsl(${hue(title)} 52% 48%);width:${size}px;height:${size}px` }, letter);
  const site = feed.site_url || feed.feed_site_url || feed.url;
  try {
    const img = h('img', { src: new URL('/favicon.ico', site).href, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer' });
    img.addEventListener('load', () => {
      if (img.naturalWidth > 1) put(box, img);
    });
  } catch { /* invalid URL: keep the letter */ }
  return box;
}

// ---------- preferences & themes ----------

function currentTheme() {
  return resolveTheme(state.prefs.theme, state.customThemes);
}

function applyPrefs() {
  applyTheme(currentTheme(), state.prefs);
  document.documentElement.dataset.images = state.prefs.showImages ? 'on' : 'off';
}

let saveTimer;
function savePrefs(patch) {
  Object.assign(state.prefs, patch);
  applyPrefs();
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => api.saveSettings({ ui: state.prefs }).catch((e) => toast(e.message, 'error')), 400);
}

window.matchMedia?.('(prefers-color-scheme: dark)').addEventListener('change', () => {
  if (state.prefs.theme === 'auto') applyPrefs();
});

// ---------- navigation (sidebar) ----------

async function loadNav() {
  const [feeds, categories, stats] = await Promise.all([api.feeds(), api.categories(), api.stats()]);
  Object.assign(state, { feeds, categories, stats });
  renderNav();
  renderListHeader();
}

function navItem({ label, count, active, iconEl, onclick, more, warn, extraClass = '', title }) {
  return h(
    'button',
    { class: `nav-item ${active ? 'active' : ''} ${count ? 'has-unread' : ''} ${extraClass}`, onclick, title: title || label },
    iconEl,
    h('span', { class: 'label' }, label),
    warn ? h('span', { class: 'warn', title: warn }, icon('alert')) : null,
    count ? h('span', { class: 'count' }, count > 999 ? '999+' : count) : null,
    more ? h('span', { class: 'more', role: 'button', 'aria-label': 'More actions', tabindex: 0, onclick: (e) => { e.stopPropagation(); more(e); } }, icon('more')) : null,
  );
}

function isActive(type, id) {
  return state.view.type === type && (id === undefined || state.view.id === id);
}

function feedNav(f) {
  return navItem({
    label: f.title,
    count: f.unread_count,
    active: isActive('feed', f.id),
    iconEl: feedIcon(f),
    warn: f.last_error ? `Last refresh failed: ${f.last_error}` : null,
    onclick: () => go({ type: 'feed', id: f.id }),
    more: (e) => feedMenu(e, f),
  });
}

function renderNav() {
  const nav = $('#nav');
  const { stats, categories, feeds } = state;
  const starredCount = stats.starred || 0;
  const top = h('div', {},
    navItem({ label: 'All articles', count: stats.unread, active: isActive('all'), iconEl: icon('inbox'), onclick: () => go({ type: 'all' }) }),
    navItem({ label: 'Starred', count: starredCount, active: isActive('starred'), iconEl: icon('star'), onclick: () => go({ type: 'starred' }) }),
  );

  const sections = [];
  if (categories.length) {
    const section = h('div', { class: 'nav-section' }, h('div', { class: 'nav-section-title' }, 'Categories'));
    for (const c of categories) {
      const collapsed = state.collapsed.has(c.id);
      const chevron = h('span', { class: 'chevron-btn', role: 'button', 'aria-label': collapsed ? 'Expand' : 'Collapse', onclick: (e) => { e.stopPropagation(); toggleCollapsed(c.id); } }, icon('chevron', 'chevron'));
      const item = navItem({
        label: c.name,
        count: c.unread_count,
        active: isActive('category', c.id),
        iconEl: chevron,
        extraClass: `category ${collapsed ? 'collapsed' : ''}`,
        onclick: () => go({ type: 'category', id: c.id }),
        more: (e) => categoryMenu(e, c),
      });
      section.append(item, h('div', { class: 'category-feeds' }, feeds.filter((f) => f.category_id === c.id).map(feedNav)));
    }
    sections.push(section);
  }
  const loose = feeds.filter((f) => !f.category_id);
  if (loose.length) {
    sections.push(h('div', { class: 'nav-section' }, h('div', { class: 'nav-section-title' }, categories.length ? 'Uncategorized' : 'Feeds'), loose.map(feedNav)));
  }
  if (!feeds.length) {
    sections.push(h('div', { class: 'list-message small' }, h('p', {}, 'No subscriptions yet.'), h('button', { class: 'btn primary', onclick: addFeedModal }, icon('plus'), 'Add your first feed')));
  }
  put(nav, top, ...sections);
}

function toggleCollapsed(id) {
  if (state.collapsed.has(id)) state.collapsed.delete(id);
  else state.collapsed.add(id);
  try {
    localStorage.setItem('openrss.collapsed', JSON.stringify([...state.collapsed]));
  } catch { /* ignore */ }
  renderNav();
}

// ---------- routing ----------

function viewToHash(v) {
  return v.type === 'feed' || v.type === 'category' ? `#/${v.type}/${v.id}` : `#/${v.type}`;
}

function hashToView() {
  const [, type, id] = location.hash.split('/');
  if ((type === 'feed' || type === 'category') && Number(id)) return { type, id: Number(id) };
  if (type === 'starred') return { type };
  return { type: 'all' };
}

function go(view) {
  closeNav();
  if (location.hash !== viewToHash(view)) location.hash = viewToHash(view);
  else route();
}

function route() {
  state.view = hashToView();
  state.selectedId = null;
  renderNav();
  renderListHeader();
  loadArticles(true);
  showPane('list');
}

// ---------- article list ----------

function viewTitle() {
  const v = state.view;
  if (v.type === 'starred') return { title: 'Starred', sub: plural(state.stats.starred || 0, 'article') };
  if (v.type === 'feed') {
    const f = state.feeds.find((x) => x.id === v.id);
    return { title: f?.title || 'Feed', sub: f ? `${f.unread_count} unread · ${hostname(f.site_url || f.url)}` : '' };
  }
  if (v.type === 'category') {
    const c = state.categories.find((x) => x.id === v.id);
    return { title: c?.name || 'Category', sub: c ? `${c.unread_count} unread · ${plural(c.feed_count, 'feed')}` : '' };
  }
  return { title: 'All articles', sub: `${state.stats.unread ?? 0} unread · ${plural(state.stats.feeds ?? 0, 'feed')}` };
}

function renderListHeader() {
  const { title, sub } = viewTitle();
  $('#view-title').textContent = title;
  $('#view-subtitle').textContent = sub;
  document.title = state.stats.unread ? `(${state.stats.unread}) OpenRSS` : 'OpenRSS';
  const starred = state.view.type === 'starred';
  for (const b of document.querySelectorAll('[data-filter]')) {
    b.setAttribute('aria-selected', String(b.dataset.filter === state.prefs.filter));
  }
  $('.segmented').hidden = starred;
}

function queryParams(offset) {
  const v = state.view;
  return {
    feed_id: v.type === 'feed' ? v.id : undefined,
    category: v.type === 'category' ? v.id : undefined,
    starred: v.type === 'starred' || undefined,
    status: v.type !== 'starred' && state.prefs.filter === 'unread' && !state.search ? 'unread' : 'all',
    q: state.search || undefined,
    limit: 40,
    offset,
  };
}

async function loadArticles(reset = false) {
  if (state.loading && !reset) return;
  const id = ++state.requestId;
  state.loading = true;
  const list = $('#article-list');
  if (reset) {
    state.articles = [];
    state.total = 0;
    list.scrollTop = 0;
    put(list, ...Array.from({ length: 6 }, () => h('div', { class: 'skeleton' }, h('i'), h('i'), h('i'))));
  } else {
    list.append(h('div', { class: 'list-loading' }, 'Loading…'));
  }
  try {
    const page = await api.articles(queryParams(state.articles.length));
    if (id !== state.requestId) return; // a newer request superseded this one
    state.articles.push(...page.articles);
    state.total = page.total;
    renderList();
  } catch (e) {
    if (id === state.requestId) put(list, listMessage('alert', 'Could not load articles', e.message));
  } finally {
    if (id === state.requestId) state.loading = false;
  }
}

function listMessage(iconName, title, text, action) {
  return h('div', { class: 'list-message' }, icon(iconName), h('h3', {}, title), text ? h('p', {}, text) : null, action || null);
}

function articleItem(a) {
  const thumb = state.prefs.showImages && a.image_url
    ? h('img', { class: 'thumb', src: a.image_url, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer', onerror: (e) => e.target.remove() })
    : null;
  return h('div', {
    class: `article-item ${a.is_read ? '' : 'unread'} ${a.id === state.selectedId ? 'active' : ''}`,
    role: 'listitem',
    dataset: { id: a.id },
    onclick: () => openArticle(a.id),
  },
  h('div', { class: 'meta' },
    h('span', { class: 'feed' }, a.feed_title),
    h('span', {}, '·'),
    h('time', { datetime: a.published_at, title: fullDate(a.published_at) }, timeAgo(a.published_at)),
    a.is_starred ? h('span', { class: 'star', 'aria-label': 'Starred' }, icon('star')) : null,
  ),
  h('h3', {}, a.title || '(untitled)'),
  a.summary ? h('p', {}, a.summary) : null,
  thumb);
}

function renderList() {
  const list = $('#article-list');
  if (!state.articles.length) {
    let msg;
    if (state.search) msg = listMessage('search', 'No matches', `Nothing found for “${state.search}”.`);
    else if (!state.feeds.length) msg = listMessage('rss', 'Welcome to OpenRSS', 'Subscribe to a site to start reading.', h('button', { class: 'btn primary', onclick: addFeedModal }, icon('plus'), 'Add a feed'));
    else if (state.view.type === 'starred') msg = listMessage('star', 'No starred articles', 'Press s while reading to keep an article here.');
    else if (state.prefs.filter === 'unread') msg = listMessage('check', 'All caught up', 'No unread articles here.', h('button', { class: 'btn', onclick: () => setFilter('all') }, 'Show all articles'));
    else msg = listMessage('inbox', 'No articles yet', 'New articles appear after the next refresh.');
    put(list, msg);
    return;
  }
  put(list, ...state.articles.map(articleItem));
}

function updateListItem(a) {
  const el = $(`.article-item[data-id="${a.id}"]`);
  if (el) el.replaceWith(articleItem(a));
}

function setFilter(filter) {
  savePrefs({ filter });
  renderListHeader();
  loadArticles(true);
}

// ---------- reader ----------

function showPane(pane) {
  $('#app').dataset.pane = pane;
}

async function openArticle(id) {
  state.selectedId = id;
  for (const el of document.querySelectorAll('.article-item.active')) el.classList.remove('active');
  const item = $(`.article-item[data-id="${id}"]`);
  item?.classList.add('active');
  item?.scrollIntoView({ block: 'nearest' });
  showPane('reader');
  try {
    const article = await api.article(id);
    if (state.selectedId !== id) return;
    state.current = article;
    renderReader(article);
    if (state.prefs.markReadOnOpen && !article.is_read) setRead(article, true);
  } catch (e) {
    toast(e.message, 'error');
  }
}

function readingTime(html) {
  const words = (html.replace(/<[^>]+>/g, ' ').match(/\S+/g) || []).length;
  return Math.max(1, Math.round(words / 230));
}

function renderReader(a) {
  $('#reader-empty').hidden = true;
  const reader = $('#reader');
  reader.hidden = false;
  const feed = state.feeds.find((f) => f.id === a.feed_id) || { title: a.feed_title, site_url: a.feed_site_url };

  const starBtn = h('button', { class: `icon-btn ${a.is_starred ? 'on' : ''}`, title: a.is_starred ? 'Unstar (s)' : 'Star (s)', onclick: () => setStarred(a, !a.is_starred) }, icon('star'));
  if (a.is_starred) starBtn.querySelector('svg').style.fill = 'currentColor';
  const readBtn = h('button', { class: 'icon-btn', title: a.is_read ? 'Mark as unread (m)' : 'Mark as read (m)', onclick: () => setRead(a, !a.is_read) }, icon(a.is_read ? 'circle' : 'check'));
  const progress = h('div', { class: 'reader-progress' });
  const bar = h('div', { class: 'reader-bar' },
    h('button', { class: 'icon-btn back', title: 'Back', onclick: () => showPane('list') }, icon('chevron-left')),
    h('button', { class: 'icon-btn', title: 'Previous (k)', onclick: () => move(-1) }, icon('chevron-up')),
    h('button', { class: 'icon-btn', title: 'Next (j)', onclick: () => move(1) }, icon('chevron-down')),
    h('div', { class: 'spacer' }),
    readBtn,
    starBtn,
    a.url ? h('a', { class: 'icon-btn', href: a.url, target: '_blank', rel: 'noopener noreferrer', title: 'Open original (v)' }, icon('external')) : null,
    progress,
  );

  const content = h('div', { class: 'content' });
  // Article HTML is sanitized by the server (ammonia) before it is stored.
  content.innerHTML = a.content || `<p>${a.summary ? '' : 'This article has no content.'}</p>`;
  if (!a.content && a.summary) content.firstElementChild.textContent = a.summary;

  const titleEl = a.url
    ? h('a', { href: a.url, target: '_blank', rel: 'noopener noreferrer' }, a.title || '(untitled)')
    : (a.title || '(untitled)');

  put(reader,
    bar,
    h('div', { class: 'reader-body' },
      h('header', { class: 'reader-head' },
        h('a', { class: 'reader-feed', href: `#/feed/${a.feed_id}` }, feedIcon(feed, 20), a.feed_title),
        h('h1', { class: 'reader-title' }, titleEl),
        h('div', { class: 'reader-byline' },
          a.author ? h('span', {}, a.author) : null,
          h('time', { datetime: a.published_at }, fullDate(a.published_at)),
          h('span', {}, `${readingTime(a.content || a.summary || '')} min read`),
        ),
      ),
      content,
      h('footer', { class: 'reader-foot' },
        a.url ? h('a', { class: 'btn', href: a.url, target: '_blank', rel: 'noopener noreferrer' }, icon('external'), 'Read on ', hostname(a.url)) : null,
        h('button', { class: 'btn', onclick: () => move(1) }, 'Next article', icon('chevron-down')),
      ),
    ),
  );
  const pane = $('#reader-pane');
  pane.scrollTop = 0;
  pane.onscroll = () => {
    bar.classList.toggle('scrolled', pane.scrollTop > 4);
    const max = pane.scrollHeight - pane.clientHeight;
    progress.style.width = `${max > 0 ? Math.min(100, (pane.scrollTop / max) * 100) : 0}%`;
  };
}

function adjustUnread(feedId, delta) {
  const f = state.feeds.find((x) => x.id === feedId);
  if (f) {
    f.unread_count = Math.max(0, f.unread_count + delta);
    const c = state.categories.find((x) => x.id === f.category_id);
    if (c) c.unread_count = Math.max(0, c.unread_count + delta);
  }
  state.stats.unread = Math.max(0, (state.stats.unread || 0) + delta);
  renderNav();
  renderListHeader();
}

function syncArticle(a, patch) {
  Object.assign(a, patch);
  const inList = state.articles.find((x) => x.id === a.id);
  if (inList && inList !== a) Object.assign(inList, patch);
  if (inList) updateListItem(inList);
  if (state.current?.id === a.id) {
    Object.assign(state.current, patch);
    renderReaderBarOnly();
  }
}

function renderReaderBarOnly() {
  // Re-render while keeping the scroll position.
  const pane = $('#reader-pane');
  const top = pane.scrollTop;
  renderReader(state.current);
  pane.scrollTop = top;
}

async function setRead(a, read) {
  if (a.is_read === read) return;
  syncArticle(a, { is_read: read });
  adjustUnread(a.feed_id, read ? -1 : 1);
  try {
    await api.updateArticle(a.id, { read });
  } catch (e) {
    syncArticle(a, { is_read: !read });
    adjustUnread(a.feed_id, read ? 1 : -1);
    toast(e.message, 'error');
  }
}

async function setStarred(a, starred) {
  syncArticle(a, { is_starred: starred });
  state.stats.starred = Math.max(0, (state.stats.starred || 0) + (starred ? 1 : -1));
  renderNav();
  try {
    await api.updateArticle(a.id, { starred });
    toast(starred ? 'Starred' : 'Removed from starred');
  } catch (e) {
    syncArticle(a, { is_starred: !starred });
    toast(e.message, 'error');
  }
}

function move(delta) {
  if (!state.articles.length) return;
  const idx = state.articles.findIndex((a) => a.id === state.selectedId);
  const next = idx === -1 ? (delta > 0 ? 0 : -1) : idx + delta;
  if (next < 0) return;
  if (next >= state.articles.length) {
    if (state.articles.length < state.total) loadArticles().then(() => state.articles[next] && openArticle(state.articles[next].id));
    else toast('No more articles');
    return;
  }
  openArticle(state.articles[next].id);
}

// ---------- actions ----------

async function refreshAll(btn = $('#refresh-btn')) {
  btn.classList.add('spin');
  btn.disabled = true;
  try {
    const r = await api.refreshAll();
    toast(r.added ? `${plural(r.added, 'new article')}` : 'No new articles' + (r.errors ? ` · ${plural(r.errors, 'feed')} failed` : ''));
    await loadNav();
    loadArticles(true);
  } catch (e) {
    toast(e.message, 'error');
  } finally {
    btn.classList.remove('spin');
    btn.disabled = false;
  }
}

async function markAllRead() {
  const v = state.view;
  if (v.type === 'starred') return;
  const body = {};
  if (v.type === 'feed') body.feed_id = v.id;
  if (v.type === 'category') body.category = String(v.id);
  // Only mark what was loaded at the time, so new arrivals stay unread.
  if (state.articles[0] && !state.search) body.before = state.articles[0].published_at;
  try {
    const r = await api.markAllRead(body);
    toast(r.updated ? `Marked ${plural(r.updated, 'article')} as read` : 'Nothing to mark');
    await loadNav();
    if (state.prefs.filter === 'unread') loadArticles(true);
    else {
      for (const a of state.articles) a.is_read = true;
      renderList();
    }
  } catch (e) {
    toast(e.message, 'error');
  }
}

// ---------- menus ----------

function openMenu(event, items) {
  closeMenu();
  const menu = h('div', { class: 'menu', role: 'menu' }, items.map((it) => (it === '-' ? h('hr') : h('button', { class: it.danger ? 'danger' : '', role: 'menuitem', onclick: () => { closeMenu(); it.action(); } }, icon(it.icon), it.label))));
  document.body.append(menu);
  const r = event.target.getBoundingClientRect();
  const left = Math.min(r.left, window.innerWidth - menu.offsetWidth - 8);
  const top = r.bottom + menu.offsetHeight + 8 > window.innerHeight ? r.top - menu.offsetHeight - 4 : r.bottom + 4;
  Object.assign(menu.style, { left: `${Math.max(8, left)}px`, top: `${Math.max(8, top)}px` });
  menu.querySelector('button')?.focus();
  setTimeout(() => document.addEventListener('click', closeMenu, { once: true }));
}

function closeMenu() {
  document.querySelector('.menu')?.remove();
}

function feedMenu(e, f) {
  openMenu(e, [
    { icon: 'edit', label: 'Edit feed', action: () => editFeedModal(f) },
    { icon: 'refresh', label: 'Refresh', action: async () => {
      try {
        const r = await api.refreshFeed(f.id);
        toast(r.error ? `Refresh failed: ${r.error}` : `${plural(r.added, 'new article')}`, r.error ? 'error' : 'info');
        await loadNav();
        if (isActive('feed', f.id)) loadArticles(true);
      } catch (err) {
        toast(err.message, 'error');
      }
    } },
    { icon: 'check-all', label: 'Mark all as read', action: async () => {
      await api.markAllRead({ feed_id: f.id });
      await loadNav();
      loadArticles(true);
    } },
    f.site_url ? { icon: 'external', label: 'Open website', action: () => window.open(f.site_url, '_blank', 'noopener') } : null,
    '-',
    { icon: 'trash', label: 'Unsubscribe', danger: true, action: () => confirmModal(`Unsubscribe from “${f.title}”?`, 'Its articles, including starred ones, will be deleted.', 'Unsubscribe', async () => {
      await api.deleteFeed(f.id);
      toast('Unsubscribed');
      if (isActive('feed', f.id)) go({ type: 'all' });
      await loadNav();
    }) },
  ].filter(Boolean));
}

function categoryMenu(e, c) {
  openMenu(e, [
    { icon: 'edit', label: 'Rename', action: () => promptModal('Rename category', 'Name', c.name, async (name) => {
      await api.updateCategory(c.id, { name });
      await loadNav();
    }) },
    { icon: 'check-all', label: 'Mark all as read', action: async () => {
      await api.markAllRead({ category: String(c.id) });
      await loadNav();
      loadArticles(true);
    } },
    '-',
    { icon: 'trash', label: 'Delete category', danger: true, action: () => confirmModal(`Delete “${c.name}”?`, 'Its feeds are kept and become uncategorized.', 'Delete', async () => {
      await api.deleteCategory(c.id);
      if (isActive('category', c.id)) go({ type: 'all' });
      await loadNav();
    }) },
  ]);
}

// ---------- modals ----------

const modal = $('#modal');
let modalCleanup = null;

function openModal({ title, body, footer, wide = false, onClose }) {
  modalCleanup?.();
  modalCleanup = onClose || null;
  modal.className = `modal ${wide ? 'wide' : ''}`;
  put(modal,
    h('header', { class: 'modal-head' }, h('h2', {}, title), h('button', { class: 'icon-btn', 'aria-label': 'Close', onclick: closeModal }, icon('x'))),
    body,
    footer ? h('footer', { class: 'modal-foot' }, footer) : null,
  );
  if (!modal.open) modal.showModal();
  modal.querySelector('[autofocus]')?.focus();
}

function closeModal() {
  if (modal.open) modal.close();
}

modal.addEventListener('close', () => {
  const fn = modalCleanup;
  modalCleanup = null;
  fn?.();
});
modal.addEventListener('click', (e) => {
  if (e.target === modal) closeModal();
});

/** Wraps a submit handler with a busy state and inline error reporting. */
function formHandler(form, button, fn) {
  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const err = form.querySelector('.error-text');
    err.hidden = true;
    button.disabled = true;
    try {
      await fn(new FormData(form));
      closeModal();
    } catch (ex) {
      err.textContent = ex.message;
      err.hidden = false;
    } finally {
      button.disabled = false;
    }
  });
}

function categoryOptions(selectedId) {
  return [
    h('option', { value: '' }, 'No category'),
    ...state.categories.map((c) => h('option', { value: c.name, selected: c.id === selectedId }, c.name)),
    h('option', { value: '__new__' }, 'New category…'),
  ];
}

function categoryField(selectedId) {
  const select = h('select', { class: 'input', name: 'category' }, categoryOptions(selectedId));
  const newInput = h('input', { class: 'input', name: 'new_category', placeholder: 'Category name', hidden: true });
  select.addEventListener('change', () => {
    newInput.hidden = select.value !== '__new__';
    if (!newInput.hidden) newInput.focus();
  });
  return h('label', { class: 'field' }, h('span', {}, 'Category'), select, newInput);
}

const chosenCategory = (data) => (data.get('category') === '__new__' ? data.get('new_category') : data.get('category')) || '';

function addFeedModal() {
  const submit = h('button', { class: 'btn primary', type: 'submit', form: 'add-feed-form' }, 'Subscribe');
  const defaultCategory = state.view.type === 'category' ? state.view.id : null;
  const form = h('form', { id: 'add-feed-form', class: 'modal-body' },
    h('p', { class: 'error-text', hidden: true }),
    h('label', { class: 'field' }, h('span', {}, 'Website or feed URL'),
      h('input', { class: 'input', name: 'url', type: 'text', inputmode: 'url', placeholder: 'https://example.com', required: true, autofocus: true, autocomplete: 'off' }),
      h('span', { class: 'hint' }, 'Paste any site address — its RSS, Atom or JSON feed is found automatically.')),
    categoryField(defaultCategory),
    h('label', { class: 'field' }, h('span', {}, 'Title (optional)'), h('input', { class: 'input', name: 'title', placeholder: 'Use the feed’s own title' })),
  );
  formHandler(form, submit, async (data) => {
    submit.textContent = 'Subscribing…';
    try {
      const r = await api.addFeed({ url: data.get('url'), category: chosenCategory(data) || undefined, title: data.get('title') || undefined });
      toast(`Subscribed to ${r.feed.title}`);
      await loadNav();
      go({ type: 'feed', id: r.feed.id });
    } finally {
      submit.textContent = 'Subscribe';
    }
  });
  openModal({ title: 'Add a feed', body: form, footer: [h('button', { class: 'btn', type: 'button', onclick: closeModal }, 'Cancel'), submit] });
}

function editFeedModal(f) {
  const submit = h('button', { class: 'btn primary', type: 'submit', form: 'edit-feed-form' }, 'Save');
  const form = h('form', { id: 'edit-feed-form', class: 'modal-body' },
    h('p', { class: 'error-text', hidden: true }),
    h('label', { class: 'field' }, h('span', {}, 'Title'), h('input', { class: 'input', name: 'title', value: f.custom_title || '', placeholder: f.title, autofocus: true })),
    categoryField(f.category_id),
    h('label', { class: 'field' }, h('span', {}, 'Feed URL'), h('input', { class: 'input', name: 'url', value: f.url, required: true })),
    f.last_error ? h('p', { class: 'hint error-text' }, `Last refresh failed: ${f.last_error}`) : null,
    h('p', { class: 'muted small' }, `${plural(f.article_count, 'article')} · last checked ${f.last_fetched_at ? fullDate(f.last_fetched_at) : 'never'}`),
  );
  formHandler(form, submit, async (data) => {
    await api.updateFeed(f.id, { title: data.get('title'), category: chosenCategory(data), url: data.get('url') });
    toast('Feed updated');
    await loadNav();
  });
  openModal({ title: 'Edit feed', body: form, footer: [h('button', { class: 'btn', type: 'button', onclick: closeModal }, 'Cancel'), submit] });
}

function promptModal(title, label, value, onSubmit) {
  const submit = h('button', { class: 'btn primary', type: 'submit', form: 'prompt-form' }, 'Save');
  const form = h('form', { id: 'prompt-form', class: 'modal-body' },
    h('p', { class: 'error-text', hidden: true }),
    h('label', { class: 'field' }, h('span', {}, label), h('input', { class: 'input', name: 'value', value, required: true, autofocus: true })));
  formHandler(form, submit, (data) => onSubmit(data.get('value')));
  openModal({ title, body: form, footer: [h('button', { class: 'btn', type: 'button', onclick: closeModal }, 'Cancel'), submit] });
}

function confirmModal(title, text, confirmLabel, onConfirm) {
  const btn = h('button', { class: 'btn danger', onclick: async () => {
    btn.disabled = true;
    try {
      await onConfirm();
      closeModal();
    } catch (e) {
      toast(e.message, 'error');
      btn.disabled = false;
    }
  } }, confirmLabel);
  openModal({ title, body: h('div', { class: 'modal-body' }, h('p', { class: 'muted' }, text)), footer: [h('button', { class: 'btn', onclick: closeModal, autofocus: true }, 'Cancel'), btn] });
}

// ---------- settings ----------

function settingsModal(tab = 'appearance') {
  const tabs = [
    ['appearance', 'Appearance'],
    ['theme', 'Theme editor'],
    ['subscriptions', 'Subscriptions'],
    ['agents', 'AI agents'],
    ['shortcuts', 'Shortcuts'],
  ];
  const body = h('div', { class: 'modal-body' });
  const tabBar = h('div', { class: 'tabs', role: 'tablist' }, tabs.map(([id, label]) => h('button', { role: 'tab', 'aria-selected': String(id === tab), onclick: () => settingsModal(id) }, label)));
  const render = { appearance: appearanceTab, theme: themeEditorTab, subscriptions: subscriptionsTab, agents: agentsTab, shortcuts: shortcutsTab }[tab];
  openModal({ title: 'Settings', body: h('div', { style: 'display:contents' }, tabBar, body), wide: true });
  render(body);
}

function themeCard(key, theme, tag) {
  const v = theme.vars;
  const selected = state.prefs.theme === key;
  const line = (color, w) => h('i', { style: `background:${color};width:${w}%` });
  return h('button', { class: `theme-card ${selected ? 'selected' : ''}`, onclick: () => { savePrefs({ theme: key }); settingsModal('appearance'); } },
    h('div', { class: 'preview' },
      h('div', { style: `background:${v['--bg-sidebar']}` }, line(v['--text-faint'], 80), line(v['--accent'], 60), line(v['--text-faint'], 70)),
      h('div', { style: `background:${v['--bg-list']}` }, line(v['--text'], 90), line(v['--text-faint'], 70), line(v['--text'], 80), line(v['--text-faint'], 60)),
      h('div', { style: `background:${v['--bg-reader']}` }, line(v['--text'], 70), line(v['--text-muted'], 95), line(v['--text-muted'], 90), line(v['--link'], 40))),
    h('div', { class: 'name' }, theme.name, h('span', { class: 'tag' }, tag)));
}

function optionRow(label, hint, control) {
  return h('div', { class: 'option-row' }, h('div', { class: 'label' }, label, hint ? h('small', {}, hint) : null), control);
}

function segmented(options, value, onChange) {
  const wrap = h('div', { class: 'segmented' });
  for (const [v, label] of options) {
    wrap.append(h('button', { 'aria-selected': String(v === value), style: 'padding:5px 10px', onclick: () => {
      for (const b of wrap.children) b.setAttribute('aria-selected', 'false');
      wrap.querySelector(`[data-v="${v}"]`).setAttribute('aria-selected', 'true');
      onChange(v);
    }, dataset: { v } }, label));
  }
  return wrap;
}

function toggle(checked, onChange) {
  return h('label', { class: 'switch' }, h('input', { type: 'checkbox', checked, onchange: (e) => onChange(e.target.checked) }));
}

function appearanceTab(body) {
  const auto = resolveTheme('auto');
  const cards = [
    themeCard('auto', { ...auto, name: 'System' }, 'auto'),
    ...Object.entries(BUILTIN_THEMES).map(([k, t]) => themeCard(k, t, t.base)),
    ...state.customThemes.map((t) => themeCard(`custom:${t.id}`, resolveTheme(`custom:${t.id}`, state.customThemes), 'custom')),
  ];
  const range = (key, min, max, step, fmt) => {
    const out = h('span', { class: 'muted small', style: 'width:42px;text-align:right' }, fmt(state.prefs[key]));
    return h('div', { style: 'display:flex;align-items:center;gap:10px' },
      h('input', { type: 'range', min, max, step, value: state.prefs[key], oninput: (e) => { out.textContent = fmt(Number(e.target.value)); savePrefs({ [key]: Number(e.target.value) }); } }), out);
  };
  put(body,
    h('div', { class: 'section-title' }, 'Theme'),
    h('div', { class: 'theme-grid' }, cards),
    h('p', { class: 'muted small' }, 'Want your own colors? Open the ', h('a', { href: '#', onclick: (e) => { e.preventDefault(); settingsModal('theme'); } }, 'theme editor'), '.'),
    h('div', { class: 'section-title' }, 'Reading'),
    optionRow('Typeface', null, segmented(Object.entries(FONTS).map(([k, f]) => [k, f.label]), state.prefs.font, (font) => savePrefs({ font }))),
    optionRow('Text size', null, range('fontSize', 14, 26, 1, (v) => `${v}px`)),
    optionRow('Line spacing', null, range('lineHeight', 1.3, 2.1, 0.05, (v) => v.toFixed(2))),
    optionRow('Line width', null, segmented(Object.keys(WIDTHS).map((k) => [k, k[0].toUpperCase() + k.slice(1)]), state.prefs.width, (width) => savePrefs({ width }))),
    h('div', { class: 'section-title' }, 'Article list'),
    optionRow('Density', null, segmented([['comfortable', 'Comfortable'], ['compact', 'Compact']], state.prefs.density, (density) => savePrefs({ density }))),
    optionRow('Show thumbnails', null, toggle(state.prefs.showImages, (showImages) => { savePrefs({ showImages }); renderList(); })),
    optionRow('Mark as read when opened', 'Otherwise press m to mark articles read.', toggle(state.prefs.markReadOnOpen, (markReadOnOpen) => savePrefs({ markReadOnOpen }))),
  );
}

function themeEditorTab(body, editing = null) {
  // Start from the active custom theme, or a copy of the active built-in one.
  const activeCustom = state.customThemes.find((t) => `custom:${t.id}` === state.prefs.theme);
  const source = editing || activeCustom;
  const start = resolveTheme(state.prefs.theme, state.customThemes);
  const draft = source
    ? { id: source.id, name: source.name, base: source.base, vars: { ...start.vars, ...source.vars }, custom_css: source.custom_css || '' }
    : { id: null, name: `My ${start.name}`, base: start.base, vars: { ...start.vars }, custom_css: '' };
  let saved = false;
  const preview = () => applyTheme(draft, state.prefs);
  modalCleanup = () => {
    if (!saved) applyPrefs();
  };

  const toHex = (v) => (/^#[0-9a-f]{6}$/i.test(v) ? v : /^#[0-9a-f]{3}$/i.test(v) ? `#${[...v.slice(1)].map((c) => c + c).join('')}` : '#888888');
  const colors = h('div', { class: 'color-list' }, THEME_VARS.map(({ key, label }) => h('label', { class: 'color-field' },
    h('input', { type: 'color', value: toHex(draft.vars[key] || ''), oninput: (e) => { draft.vars[key] = e.target.value; preview(); } }), label)));

  const baseSelect = h('select', { class: 'input', onchange: (e) => { draft.base = e.target.value; preview(); } },
    h('option', { value: 'light', selected: draft.base === 'light' }, 'Light'), h('option', { value: 'dark', selected: draft.base === 'dark' }, 'Dark'));
  const nameInput = h('input', { class: 'input', value: draft.name, oninput: (e) => { draft.name = e.target.value; } });
  const css = h('textarea', { class: 'input', spellcheck: 'false', placeholder: '/* Any CSS, e.g. */\n.reader-title { font-style: italic; }\n.content p:first-of-type::first-letter { font-size: 3em; float: left; line-height: 1; margin-right: .08em; }', oninput: (e) => { draft.custom_css = e.target.value; preview(); } });
  css.value = draft.custom_css;

  const startFrom = h('select', { class: 'input', onchange: (e) => {
    const t = BUILTIN_THEMES[e.target.value];
    if (!t) return;
    Object.assign(draft.vars, t.vars);
    draft.base = t.base;
    themeEditorTabRefresh();
  } }, h('option', { value: '' }, 'Reset colors from…'), Object.entries(BUILTIN_THEMES).map(([k, t]) => h('option', { value: k }, t.name)));

  const themePicker = state.customThemes.length
    ? h('select', { class: 'input', onchange: (e) => {
      const t = state.customThemes.find((x) => String(x.id) === e.target.value);
      themeEditorTab(body, t || null);
      if (!t) preview();
    } }, h('option', { value: '' }, 'New theme'), state.customThemes.map((t) => h('option', { value: t.id, selected: t.id === draft.id }, t.name)))
    : null;

  const err = h('p', { class: 'error-text', hidden: true });
  const save = h('button', { class: 'btn primary', onclick: async () => {
    err.hidden = true;
    try {
      const payload = { name: draft.name, base: draft.base, vars: draft.vars, custom_css: draft.custom_css };
      const t = draft.id ? await api.updateTheme(draft.id, payload) : await api.createTheme(payload);
      state.customThemes = await api.themes();
      saved = true;
      savePrefs({ theme: `custom:${t.id}` });
      toast(`Theme “${t.name}” saved`);
      themeEditorTab(body, t);
    } catch (e) {
      err.textContent = e.message;
      err.hidden = false;
    }
  } }, 'Save & apply');
  const del = draft.id ? h('button', { class: 'btn danger', onclick: async () => {
    await api.deleteTheme(draft.id);
    state.customThemes = await api.themes();
    if (state.prefs.theme === `custom:${draft.id}`) savePrefs({ theme: 'auto' });
    saved = true;
    toast('Theme deleted');
    themeEditorTab(body);
  } }, icon('trash'), 'Delete') : null;

  const exportBtn = h('button', { class: 'btn', onclick: () => {
    const blob = new Blob([JSON.stringify({ name: draft.name, base: draft.base, vars: draft.vars, custom_css: draft.custom_css }, null, 2)], { type: 'application/json' });
    h('a', { href: URL.createObjectURL(blob), download: `${draft.name.replace(/[^\w-]+/g, '-').toLowerCase() || 'theme'}.json` }).click();
  } }, icon('download'), 'Export');
  const importInput = h('input', { type: 'file', accept: 'application/json,.json', hidden: true, onchange: async (e) => {
    try {
      const t = JSON.parse(await e.target.files[0].text());
      Object.assign(draft, { id: null, name: t.name || 'Imported theme', base: t.base === 'dark' ? 'dark' : 'light', vars: { ...draft.vars, ...(t.vars || {}) }, custom_css: t.custom_css || '' });
      themeEditorTabRefresh();
      toast('Theme imported — save to keep it');
    } catch {
      toast('That file is not a valid theme', 'error');
    }
  } });
  const importBtn = h('button', { class: 'btn', onclick: () => importInput.click() }, icon('upload'), 'Import');

  function themeEditorTabRefresh() {
    for (const [i, { key }] of THEME_VARS.entries()) colors.children[i].querySelector('input').value = toHex(draft.vars[key] || '');
    baseSelect.value = draft.base;
    nameInput.value = draft.name;
    css.value = draft.custom_css;
    preview();
  }

  put(body,
    h('p', { class: 'muted small', style: 'margin-top:0' }, 'Changes preview live across the whole app. Save to keep them; close to discard.'),
    themePicker ? h('label', { class: 'field' }, h('span', {}, 'Editing'), themePicker) : null,
    h('div', { class: 'row' }, h('label', { class: 'field' }, h('span', {}, 'Name'), nameInput), h('label', { class: 'field' }, h('span', {}, 'Base'), baseSelect)),
    h('div', { class: 'section-title' }, 'Colors'),
    h('div', { style: 'margin-bottom:12px;max-width:260px' }, startFrom),
    colors,
    h('div', { class: 'section-title' }, 'Custom CSS'),
    h('label', { class: 'field' }, css, h('span', { class: 'hint' }, 'Applied after the theme. Every color is a CSS variable, e.g. var(--accent).')),
    err,
    h('div', { style: 'display:flex;gap:8px;flex-wrap:wrap' }, save, exportBtn, importBtn, importInput, del),
  );
  preview();
}

function subscriptionsTab(body) {
  const fileInput = h('input', { type: 'file', accept: '.opml,.xml,text/xml,text/x-opml', hidden: true, onchange: async (e) => {
    const file = e.target.files[0];
    if (!file) return;
    status.textContent = 'Importing… this can take a moment for large files.';
    try {
      const r = await api.importOpml(await file.text());
      status.textContent = `Found ${r.found}: imported ${r.imported}, already subscribed ${r.already_subscribed}, failed ${r.failed.length}.`;
      if (r.failed.length) status.append(h('ul', { class: 'small' }, r.failed.map((f) => h('li', {}, `${f.url} — ${f.error}`))));
      await loadNav();
      loadArticles(true);
    } catch (ex) {
      status.textContent = ex.message;
    }
  } });
  const status = h('div', { class: 'muted small', style: 'margin-top:12px' });
  const errors = state.feeds.filter((f) => f.last_error);
  put(body,
    h('div', { class: 'section-title' }, 'OPML'),
    h('p', { class: 'muted' }, 'Move your subscriptions from or to another reader.'),
    h('div', { style: 'display:flex;gap:8px;flex-wrap:wrap' },
      h('button', { class: 'btn', onclick: () => fileInput.click() }, icon('upload'), 'Import OPML'),
      h('a', { class: 'btn', href: '/api/opml', download: 'openrss.opml' }, icon('download'), 'Export OPML'),
      fileInput),
    status,
    h('div', { class: 'section-title' }, 'Refresh'),
    h('p', { class: 'muted' }, `Feeds refresh automatically every ${state.info.refresh_interval_minutes || 30} minutes. ${plural(state.feeds.length, 'feed')}, ${plural(state.stats.articles || 0, 'article')} stored.`),
    errors.length ? h('div', {},
      h('div', { class: 'section-title' }, `Feeds with errors (${errors.length})`),
      errors.map((f) => optionRow(f.title, f.last_error, h('button', { class: 'btn', onclick: () => editFeedModal(f) }, 'Edit')))) : null,
  );
}

function codeBlock(text) {
  const copy = h('button', { class: 'icon-btn copy', title: 'Copy', onclick: async () => {
    try {
      await navigator.clipboard.writeText(text);
      toast('Copied');
    } catch {
      toast('Copy failed — select the text instead', 'error');
    }
  } }, icon('copy'));
  return h('div', { class: 'code-block' }, text, copy);
}

function agentsTab(body) {
  const url = `${location.origin}/mcp`;
  const { mcp_enabled: enabled, mcp_requires_token: token } = state.info;
  if (!enabled) {
    put(body, h('p', { class: 'muted' }, 'The MCP endpoint is disabled on this server (MCP_ENABLED=false).'));
    return;
  }
  const header = token ? ' --header "Authorization: Bearer $OPENRSS_API_TOKEN"' : '';
  const json = { mcpServers: { openrss: { type: 'http', url, ...(token ? { headers: { Authorization: 'Bearer YOUR_API_TOKEN' } } : {}) } } };
  put(body,
    h('p', { style: 'margin-top:0' }, 'OpenRSS speaks the ', h('strong', {}, 'Model Context Protocol'), ', so AI agents can subscribe to feeds, organize categories, triage and read articles for you.'),
    h('div', { class: 'section-title' }, 'Endpoint'),
    codeBlock(url),
    h('p', { class: 'muted small' }, token
      ? 'This server requires the API token configured in API_TOKEN, sent as a Bearer token.'
      : 'No API_TOKEN is configured, so the endpoint is open to anyone who can reach it. Set API_TOKEN before exposing OpenRSS to the internet.'),
    h('div', { class: 'section-title' }, 'Claude Code'),
    codeBlock(`claude mcp add --transport http openrss ${url}${header}`),
    h('div', { class: 'section-title' }, 'Other MCP clients (JSON config)'),
    codeBlock(JSON.stringify(json, null, 2)),
    h('div', { class: 'section-title' }, 'Local stdio (inside the container)'),
    codeBlock('docker exec -i openrss /openrss mcp'),
    h('div', { class: 'section-title' }, 'Things to ask your agent'),
    h('ul', { class: 'muted' },
      h('li', {}, '“Subscribe me to the Rust and Go blogs in a Programming category.”'),
      h('li', {}, '“Summarize today’s unread articles and star the ones about databases.”'),
      h('li', {}, '“Find feeds that keep failing and fix or remove them.”')),
  );
}

const SHORTCUTS = [
  ['j / k', 'Next / previous article'],
  ['m', 'Toggle read'],
  ['s', 'Toggle star'],
  ['v', 'Open original in a new tab'],
  ['/', 'Search'],
  ['u', 'Toggle unread only'],
  ['r', 'Refresh all feeds'],
  ['a', 'Add a feed'],
  ['Shift + A', 'Mark all as read'],
  ['g then a / s', 'Go to all / starred'],
  ['t', 'Cycle light & dark themes'],
  ['Esc', 'Close dialog / back to list'],
  ['?', 'Show shortcuts'],
];

function shortcutsTab(body) {
  put(body, h('dl', { class: 'shortcut-list' }, SHORTCUTS.flatMap(([k, d]) => [h('dt', {}, k.split(' ').map((p) => (['/', 'then', '+'].includes(p) && k !== '/' ? ` ${p} ` : h('kbd', {}, p)))), h('dd', {}, d)])));
}

// ---------- keyboard ----------

let pendingG = false;
function onKey(e) {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  const tag = e.target.tagName;
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || e.target.isContentEditable) {
    if (e.key === 'Escape') e.target.blur();
    return;
  }
  if (modal.open) return;
  const a = state.current && state.current.id === state.selectedId ? state.current : null;
  if (pendingG) {
    pendingG = false;
    if (e.key === 'a') go({ type: 'all' });
    if (e.key === 's') go({ type: 'starred' });
    return;
  }
  switch (e.key) {
    case 'j': case 'ArrowDown': if (e.key === 'j' || $('#app').dataset.pane === 'list') { e.preventDefault(); move(1); } break;
    case 'k': case 'ArrowUp': if (e.key === 'k' || $('#app').dataset.pane === 'list') { e.preventDefault(); move(-1); } break;
    case 'm': if (a) setRead(a, !a.is_read); break;
    case 's': if (a) setStarred(a, !a.is_starred); break;
    case 'v': if (a?.url) window.open(a.url, '_blank', 'noopener'); break;
    case '/': e.preventDefault(); $('#search').focus(); break;
    case 'u': setFilter(state.prefs.filter === 'unread' ? 'all' : 'unread'); break;
    case 'r': refreshAll(); break;
    case 'a': addFeedModal(); break;
    case 'A': markAllRead(); break;
    case 'g': pendingG = true; setTimeout(() => { pendingG = false; }, 1200); break;
    case 't': savePrefs({ theme: currentTheme().base === 'dark' ? 'daylight' : 'midnight' }); break;
    case '?': settingsModal('shortcuts'); break;
    case 'Escape': showPane('list'); closeNav(); break;
    default: return;
  }
}

// ---------- mobile nav ----------

function openNav() {
  $('#app').classList.add('nav-open');
  $('#scrim').hidden = false;
}
function closeNav() {
  $('#app').classList.remove('nav-open');
  $('#scrim').hidden = true;
}

// ---------- boot ----------

async function init() {
  hydrateIcons();
  try {
    state.collapsed = new Set(JSON.parse(localStorage.getItem('openrss.collapsed') || '[]'));
  } catch { /* ignore */ }

  $('#add-feed-btn').onclick = addFeedModal;
  $('#refresh-btn').onclick = () => refreshAll();
  $('#settings-btn').onclick = () => settingsModal();
  $('#mark-all-btn').onclick = markAllRead;
  $('#menu-btn').onclick = openNav;
  $('#scrim').onclick = closeNav;
  for (const b of document.querySelectorAll('[data-filter]')) b.onclick = () => setFilter(b.dataset.filter);
  let searchTimer;
  $('#search').addEventListener('input', (e) => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      state.search = e.target.value.trim();
      loadArticles(true);
    }, 250);
  });
  $('#article-list').addEventListener('scroll', (e) => {
    const el = e.target;
    if (el.scrollTop + el.clientHeight > el.scrollHeight - 400 && state.articles.length < state.total && !state.loading) loadArticles();
  });
  document.addEventListener('keydown', onKey);
  window.addEventListener('hashchange', route);

  try {
    const [settings, themes, info] = await Promise.all([api.settings(), api.themes(), api.info()]);
    state.prefs = { ...DEFAULT_PREFS, ...(settings.ui || {}) };
    state.customThemes = themes;
    state.info = info;
  } catch (e) {
    toast(`Could not load settings: ${e.message}`, 'error');
  }
  applyPrefs();
  await loadNav().catch((e) => toast(e.message, 'error'));
  route();

  // Keep counts fresh while the tab is open.
  setInterval(() => {
    if (document.visibilityState === 'visible') loadNav().catch(() => {});
  }, 120_000);
}

init();
