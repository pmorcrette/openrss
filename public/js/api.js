// Thin wrapper around the REST API.

async function request(method, path, body, { raw = false } = {}) {
  const opts = { method, headers: {} };
  if (body !== undefined) {
    if (typeof body === 'string') {
      opts.body = body;
      opts.headers['content-type'] = 'text/plain';
    } else {
      opts.body = JSON.stringify(body);
      opts.headers['content-type'] = 'application/json';
    }
  }
  const res = await fetch(`/api${path}`, opts);
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      message = (await res.json()).error || message;
    } catch { /* not JSON */ }
    throw new Error(message);
  }
  if (raw) return res;
  return res.status === 204 ? null : res.json();
}

const qs = (params) => {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== '' && v !== false) p.set(k, v);
  const s = p.toString();
  return s ? `?${s}` : '';
};

export const api = {
  info: () => request('GET', '/info'),
  stats: () => request('GET', '/stats'),
  feeds: () => request('GET', '/feeds'),
  addFeed: (body) => request('POST', '/feeds', body),
  updateFeed: (id, body) => request('PATCH', `/feeds/${id}`, body),
  deleteFeed: (id) => request('DELETE', `/feeds/${id}`),
  refreshFeed: (id) => request('POST', `/feeds/${id}/refresh`),
  refreshAll: () => request('POST', '/refresh'),
  categories: () => request('GET', '/categories'),
  createCategory: (name) => request('POST', '/categories', { name }),
  updateCategory: (id, body) => request('PATCH', `/categories/${id}`, body),
  deleteCategory: (id) => request('DELETE', `/categories/${id}`),
  articles: (params) => request('GET', `/articles${qs(params)}`),
  article: (id) => request('GET', `/articles/${id}`),
  updateArticle: (id, body) => request('PATCH', `/articles/${id}`, body),
  markAllRead: (body) => request('POST', '/articles/mark-all-read', body),
  importOpml: (xml) => request('POST', '/opml', xml),
  settings: () => request('GET', '/settings'),
  saveSettings: (patch) => request('PATCH', '/settings', patch),
  themes: () => request('GET', '/themes'),
  createTheme: (t) => request('POST', '/themes', t),
  updateTheme: (id, t) => request('PUT', `/themes/${id}`, t),
  deleteTheme: (id) => request('DELETE', `/themes/${id}`),
};
