'use strict';
// soundiya-tg's web panel. Everything shown comes from the speaker as data and goes into the
// page as text (textContent), never as markup.

const $ = id => document.getElementById(id);
const ICON_PLAY = '<svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M8 5.6v12.8a.6.6 0 0 0 .92.5l9.6-6.4a.6.6 0 0 0 0-1L8.92 5.1A.6.6 0 0 0 8 5.6z"/></svg>';
const ICON_PAUSE = '<svg viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M7 5h3.4v14H7zM13.6 5H17v14h-3.4z"/></svg>';
const GLYPH_NOTE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M9 18V5l11-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/></svg>';
const GLYPH_LIKES = '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 21s-7.5-4.6-9.5-9.3C1.2 8.6 3.3 5 6.8 5c2 0 3.4 1.1 4.2 2.3h2C13.8 6.1 15.2 5 17.2 5c3.5 0 5.6 3.6 4.3 6.7C19.5 16.4 12 21 12 21z"/></svg>';
const GLYPH_WAVE = '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 2l2.2 6.3L20.5 10l-6.3 2.2L12 18.5l-2.2-6.3L3.5 10l6.3-1.7z"/><path d="M19 15l.9 2.4 2.4.9-2.4.9L19 21.6l-.9-2.4-2.4-.9 2.4-.9z"/></svg>';
const GLYPH_SIMILAR = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="2"/><path d="M16.2 7.8a6 6 0 0 1 0 8.4"/><path d="M7.8 16.2a6 6 0 0 1 0-8.4"/><path d="M19 5a10 10 0 0 1 0 14"/><path d="M5 19A10 10 0 0 1 5 5"/></svg>';
// The words soundiya knows for genres (its voice commands use the same).
const GENRES = [['Всё', ''], ['Хип-хоп', 'хип-хоп'], ['Электроника', 'электроника'], ['Хаус', 'хаус'], ['Техно', 'техно'],
  ['Рок', 'рок'], ['Поп', 'поп'], ['Джаз', 'джаз'], ['Эмбиент', 'эмбиент'], ['Классика', 'классика']];
const QUEUE = 25;

let player = null;        // the last /api/state
let clock = { pos: 0, at: 0, dur: 0, running: false };
let queueSig = '';
let genre = '';
let chartTracks = [], likeTracks = [];
let signedIn = null;      // SoundCloud account on the speaker
let polling = null;

// ---- talking to the speaker ----
async function api(path, body) {
  const opts = body === undefined
    ? { cache: 'no-store', credentials: 'same-origin' }
    : { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) };
  let r;
  try { r = await fetch(path, opts); } catch (e) { return { error: 'нет связи с колонкой' }; }
  if (r.status === 401 && path !== '/api/login') { showLogin(); return { error: 'нужен PIN' }; }
  try { return await r.json(); } catch (e) { return { error: 'ошибка ' + r.status }; }
}
const cmd = body => api('/api/cmd', body);
const list = body => api('/api/list', body);

function show(el, text, isErr) {
  el.textContent = text || '';
  el.className = 'msg' + (text ? (isErr ? ' err' : ' ok') : '');
}
function busy(el, text) { el.className = 'msg'; el.textContent = text; }
async function run(el, label, body, done) {
  busy(el, label);
  const r = await cmd(body);
  if (r.error) show(el, r.error, true); else show(el, done || '');
  setTimeout(refresh, 400);
  setTimeout(refresh, 1500);
  return r;
}

function mmss(sec) {
  sec = Math.max(0, Math.floor(sec || 0));
  return Math.floor(sec / 60) + ':' + String(sec % 60).padStart(2, '0');
}
// SoundCloud covers come in sizes: the list size is small, the player wants a bigger one.
function artSize(url, size) { return url ? url.replace(/-large\.(jpg|png)$/, '-' + size + '.$1') : ''; }
function okArt(url) { return /^https:\/\/[a-z0-9.-]+\.sndcdn\.com\//.test(url || ''); }

// ---- sign-in ----
function showLogin() {
  stopPolling();
  $('app').hidden = true;
  $('login').hidden = false;
  $('state').textContent = '';
  setTimeout(() => $('pin').focus(), 50);
}
$('loginform').addEventListener('submit', async e => {
  e.preventDefault();
  const pin = $('pin').value.trim();
  if (!pin) return;
  busy($('loginmsg'), 'Проверяю…');
  const r = await api('/api/login', { pin });
  $('pin').value = '';
  if (r.error) { show($('loginmsg'), r.error, true); return; }
  show($('loginmsg'), '');
  start();
});
$('logout').addEventListener('click', async () => { await api('/api/logout', {}); showLogin(); });

// ---- the player ----
function current() { return player && player.index > 0 ? player.queue[player.index - 1] : null; }

function drawPosition() {
  const t = clock.running ? clock.pos + (Date.now() - clock.at) / 1000 : clock.pos;
  const pos = clock.dur ? Math.min(t, clock.dur) : t;
  $('pos').textContent = mmss(pos);
  const pct = clock.dur ? Math.min(100, pos / clock.dur * 100) : 0;
  if (!seekDrag) { $('progress').style.width = pct + '%'; $('seekdot').style.left = pct + '%'; }
}
setInterval(drawPosition, 500);

function renderPlayer(s) {
  player = s;
  const t = current();
  const names = { loading: 'загружаю трек…', idle: '' };
  const st = $('state');
  st.classList.toggle('busy', s.state === 'loading');
  if (s.voice) st.textContent = '🎙 голосовое сообщение';
  else if (s.state === 'loading') { st.innerHTML = '<i class="spin" aria-hidden="true"></i>'; st.append(names.loading); }
  else st.textContent = s.sleep ? 'сон через ' + s.sleep + ' мин' : '';
  document.querySelector('.bar').classList.toggle('busy', s.state === 'loading');
  $('track').textContent = t ? t.artist + ' — ' + t.title : (s.state === 'idle' ? 'Ничего не играет' : '—');
  const cover = $('cover');
  const art = t && okArt(t.art) ? artSize(t.art, 't300x300') : '';
  if (!art) { cover.hidden = true; cover.removeAttribute('src'); delete cover.dataset.src; }
  else if (cover.dataset.src !== art) {
    cover.dataset.src = art;
    cover.onload = () => { cover.hidden = false; };
    cover.onerror = () => { cover.hidden = true; };
    cover.src = art;
  }
  $('dur').textContent = mmss(t ? t.duration : 0);
  clock = { pos: s.position || 0, at: Date.now(), dur: t ? t.duration : 0, running: s.state === 'playing' };
  drawPosition();
  const want = s.state === 'playing' ? 'pause' : 'play';
  if ($('toggle').dataset.icon !== want) { $('toggle').dataset.icon = want; $('toggle').innerHTML = want === 'pause' ? ICON_PAUSE : ICON_PLAY; }
  $('repeatbtn').classList.toggle('on', !!s.repeat);
  $('openbtn').disabled = !(t && t.url);
  if (!volDragging && typeof s.volume === 'number') { $('volrange').value = s.volume; $('volval').textContent = s.volume; }
  for (const b of $('sleepseg').children) {
    const m = Number(b.dataset.min);
    b.classList.toggle('on', m === 0 ? !s.sleep : false);
  }
  $('sleeptext').textContent = s.sleep ? 'Сон через ' + s.sleep + ' мин' : 'Таймер сна';
  $('sleepbtn').classList.toggle('on', !$('sleeppop').hidden || !!s.sleep);
  renderQueue(s);
  if (signedIn !== s.signed_in) { signedIn = s.signed_in; renderForYou(); loadMine(); }
  renderAbout(s);
}

function renderQueue(s) {
  const sig = s.index + '|' + s.state + '|' + s.queue.map(q => q.id).join(',');
  if (sig === queueSig) return;
  queueSig = sig;
  const q = $('queue');
  q.replaceChildren(...s.queue.map((t, i) => trackItem(t, String(i + 1), () => jump(i + 1),
    i + 1 === s.index && s.state !== 'idle' ? 'now' : '')));
  $('qempty').style.display = s.queue.length ? 'none' : 'block';
  $('qcount').textContent = s.queue.length && s.index ? s.index + ' из ' + s.queue.length : '';
}
async function jump(n) { await run($('actmsg'), 'Включаю…', { do: 'jump', n }); showQueue(false); }

async function refresh() {
  const s = await api('/api/state');
  if (s.error) {
    if ($('app').hidden) return;
    $('state').textContent = s.error === 'нужен PIN' ? '' : s.error;
    return;
  }
  renderPlayer(s);
}
function startPolling() {
  stopPolling();
  polling = setInterval(() => { if (!document.hidden) refresh(); }, 1500);
}
function stopPolling() { if (polling) clearInterval(polling); polling = null; }
document.addEventListener('visibilitychange', () => { if (!document.hidden && polling) refresh(); });

// Buttons with data-cmd: prev, toggle, next, similar.
document.querySelectorAll('[data-cmd]').forEach(b => b.addEventListener('click', async () => {
  b.classList.add('pending');
  const labels = { similar: 'Подбираю похожие…' };
  await run($('actmsg'), labels[b.dataset.cmd] || '', { do: b.dataset.cmd });
  b.classList.remove('pending');
}));
$('repeatbtn').addEventListener('click', () => run($('actmsg'), '', { do: 'repeat', on: !(player && player.repeat) }));
$('openbtn').addEventListener('click', () => {
  const t = current();
  if (t && /^https:\/\/soundcloud\.com\//.test(t.url)) window.open(t.url, '_blank', 'noopener,noreferrer');
});

// Seek: press or drag on the bar.
let seekDrag = false;
function seekFraction(e) {
  const r = $('seek').getBoundingClientRect();
  return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
}
$('seek').addEventListener('pointerdown', e => {
  if (!clock.dur) return;
  seekDrag = true; $('seek').classList.add('drag'); $('seek').setPointerCapture(e.pointerId);
  const f = seekFraction(e); $('progress').style.width = f * 100 + '%'; $('seekdot').style.left = f * 100 + '%';
});
$('seek').addEventListener('pointermove', e => {
  if (!seekDrag) return;
  const f = seekFraction(e); $('progress').style.width = f * 100 + '%'; $('seekdot').style.left = f * 100 + '%';
  $('pos').textContent = mmss(f * clock.dur);
});
$('seek').addEventListener('pointerup', async e => {
  if (!seekDrag) return;
  seekDrag = false; $('seek').classList.remove('drag');
  const to = Math.round(seekFraction(e) * clock.dur);
  clock = { ...clock, pos: to, at: Date.now() };
  await run($('actmsg'), '', { do: 'seek', to });
});
$('seek').addEventListener('pointercancel', () => { seekDrag = false; $('seek').classList.remove('drag'); });

// Volume and the sleep timer.
let volDragging = false, volTimer = null;
$('sleepbtn').addEventListener('click', () => {
  const open = $('sleeppop').hidden;
  $('sleeppop').hidden = !open; $('sleepbtn').setAttribute('aria-expanded', String(open));
  $('sleepbtn').classList.toggle('on', open || !!(player && player.sleep));
});
$('volrange').addEventListener('input', () => {
  volDragging = true;
  $('volval').textContent = $('volrange').value;
  clearTimeout(volTimer);
  volTimer = setTimeout(async () => {
    await cmd({ do: 'volume', level: Number($('volrange').value) });
    volDragging = false;
  }, 250);
});
$('sleepseg').addEventListener('click', e => {
  const b = e.target.closest('button'); if (!b) return;
  const minutes = Number(b.dataset.min);
  run($('actmsg'), '', { do: 'sleep', minutes }, minutes ? 'Колонка уснёт через ' + minutes + ' мин' : 'Таймер выключен');
});

// The queue pops over the player.
let qTimer = null;
function showQueue(on) {
  clearTimeout(qTimer);
  $('qpop').hidden = !on; $('qbtn').setAttribute('aria-expanded', String(on)); $('qbtn').classList.toggle('on', on);
  if (on) { const now = $('queue').querySelector('li.now'); if (now) now.scrollIntoView({ block: 'center' }); }
}
// With a mouse the list opens on hover and a click keeps it open; on a touch screen a tap toggles it.
const HOVER = matchMedia('(hover: hover)').matches;
$('qbtn').addEventListener('click', () => showQueue(HOVER ? true : $('qpop').hidden));
if (HOVER) {
  $('qwrap').addEventListener('mouseenter', () => showQueue(true));
  $('qwrap').addEventListener('mouseleave', () => { qTimer = setTimeout(() => showQueue(false), 350); });
}
document.addEventListener('click', e => { if (!$('qpop').hidden && !$('qwrap').contains(e.target)) showQueue(false); });

// ---- search ----
function openSearch() { $('psearch').hidden = false; $('psearchbtn').classList.add('on'); $('psearchbtn').setAttribute('aria-expanded', 'true'); setTimeout(() => $('query').focus(), 30); }
function closeSearch() { $('psearch').hidden = true; $('psearchbtn').classList.remove('on'); $('psearchbtn').setAttribute('aria-expanded', 'false'); }
$('psearchbtn').addEventListener('click', () => { if ($('psearch').hidden) openSearch(); else closeSearch(); });
document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  if (!$('psearch').hidden) closeSearch();
  if (!$('qpop').hidden) showQueue(false);
});
$('searchform').addEventListener('submit', async e => {
  e.preventDefault();
  const q = $('query').value.trim();
  if (!q) return;
  const box = $('results');
  box.replaceChildren();
  busy($('searchmsg'), q.startsWith('https://') ? 'Открываю ссылку…' : 'Ищу…');
  const r = await list({ kind: 'find', q });
  if (r.error) { show($('searchmsg'), r.error, true); return; }
  if (!r.length) { show($('searchmsg'), 'Ничего не нашлось', true); return; }
  show($('searchmsg'), '');
  const ul = document.createElement('ul'); ul.className = 'list';
  ul.append(...r.map((t, i) => trackItem(t, mmss(t.duration), () => playFrom(r, i, $('searchmsg')))));
  box.append(ul);
});

// ---- lists, cards ----
function artBox(url, cls) {
  if (okArt(url)) {
    const img = document.createElement('img');
    img.className = cls; img.alt = ''; img.loading = 'lazy'; img.decoding = 'async';
    img.referrerPolicy = 'no-referrer';
    img.src = url;
    img.onerror = () => { const g = glyphBox(cls); img.replaceWith(g); };
    return img;
  }
  return glyphBox(cls);
}
function glyphBox(cls) { const d = document.createElement('div'); d.className = cls + ' none'; d.innerHTML = GLYPH_NOTE; return d; }

function trackItem(t, right, onclick, cls) {
  const li = document.createElement('li');
  li.className = 'tr' + (cls ? ' ' + cls : '');
  const tx = document.createElement('div'); tx.className = 't'; tx.textContent = t.title;
  const sm = document.createElement('small'); sm.textContent = t.artist; tx.append(sm);
  const d = document.createElement('div'); d.className = 'd'; d.textContent = right || '';
  li.append(artBox(t.art, 'art'), tx, d);
  li.onclick = onclick;
  return li;
}

function card(art, title, sub, onclick, glyph) {
  const b = document.createElement('button'); b.className = 'card'; b.type = 'button'; b.onclick = onclick; b.title = title;
  const c = document.createElement('div'); c.className = 'cv';
  if (okArt(art)) {
    const img = document.createElement('img'); img.alt = ''; img.loading = 'lazy'; img.referrerPolicy = 'no-referrer';
    img.src = artSize(art, 't300x300');
    img.onerror = () => { c.innerHTML = glyph || GLYPH_NOTE; };
    c.append(img);
  } else c.innerHTML = glyph || GLYPH_NOTE;
  const tb = document.createElement('b'); tb.textContent = title;
  const sm = document.createElement('small'); sm.textContent = sub || '';
  b.append(c, tb, sm);
  return b;
}

// ‹ › and «Все» next to a shelf's title, as on the author's panel.
function shelfNav(box) {
  const row = box.querySelector('.shelf'), head = box.querySelector('.whead');
  const by = d => () => row.scrollBy({ left: d * row.clientWidth * .85, behavior: 'smooth' });
  const btn = (text, label, fn, cls) => {
    const b = document.createElement('button'); b.type = 'button'; b.className = cls; b.textContent = text;
    b.setAttribute('aria-label', label); b.onclick = fn; return b;
  };
  const all = btn('Все', 'Показать все', () => {
    box.classList.toggle('all'); all.textContent = box.classList.contains('all') ? 'Свернуть' : 'Все';
  }, 'pill');
  const nav = document.createElement('div'); nav.className = 'shelfnav';
  nav.append(btn('‹', 'Назад', by(-1), 'icon sm'), btn('›', 'Дальше', by(1), 'icon sm'), all);
  head.append(nav);
}
shelfNav($('chartbox'));
shelfNav($('playlistbox'));

// Plays a listing from track `i` on: that track and the ones after it.
function playFrom(tracks, i, el) {
  const ids = tracks.slice(i, i + QUEUE).map(t => t.id);
  return run(el, 'Включаю…', { do: 'play', ids });
}

// ---- Главная ----
function renderForYou() {
  const el = $('homemsg');
  const cards = [
    card('', 'Волна', signedIn ? 'похожее на ваши лайки' : 'похожее на текущий трек', () => run(el, 'Подбираю волну…', { do: 'wave' }), GLYPH_WAVE),
  ];
  if (signedIn) cards.push(card('', 'Мои лайки', 'всё, что вы лайкнули', () => run(el, 'Включаю лайки…', { do: 'likes' }), GLYPH_LIKES));
  cards.push(card('', 'Похожее', 'на то, что играет', () => run(el, 'Подбираю похожие…', { do: 'similar' }), GLYPH_SIMILAR));
  cards.forEach(c => c.classList.add('big'));
  $('foryou').replaceChildren(...cards);
}

function renderGenres() {
  $('genres').replaceChildren(...GENRES.map(([label, word]) => {
    const b = document.createElement('button'); b.type = 'button'; b.textContent = label;
    b.classList.toggle('on', word === genre);
    b.onclick = () => { genre = word; renderGenres(); loadChart(); };
    return b;
  }));
}
async function loadChart() {
  $('chartempty').hidden = false; $('chartempty').textContent = 'Загружаю…';
  $('chart').replaceChildren();
  const want = genre;
  const r = await list({ kind: 'chart', trending: true, genre: want || null });
  if (want !== genre) return;
  if (r.error) { $('chartempty').textContent = r.error; return; }
  chartTracks = r;
  $('chartempty').hidden = r.length > 0; $('chartempty').textContent = 'Пусто';
  $('chart').replaceChildren(...r.map((t, i) => card(t.art, t.title, t.artist, () => playFrom(chartTracks, i, $('homemsg')))));
}
$('charttitle').addEventListener('click', () => run($('homemsg'), 'Включаю…', { do: 'chart', trending: true, genre: genre || null }));

// ---- Моя музыка ----
async function loadMine() {
  $('mineout').hidden = signedIn !== false;
  $('minein').hidden = !signedIn;
  if (!signedIn) return;
  $('playlistsempty').hidden = false; $('playlistsempty').textContent = 'Загружаю…';
  $('likesempty').hidden = false; $('likesempty').textContent = 'Загружаю…';
  const [pl, likes] = await Promise.all([list({ kind: 'playlists' }), list({ kind: 'likes' })]);
  if (pl.error) $('playlistsempty').textContent = pl.error;
  else {
    $('playlistsempty').hidden = pl.length > 0; $('playlistsempty').textContent = 'Плейлистов нет';
    $('playlists').replaceChildren(...pl.map(p => card(p.art, p.title || '(без названия)', p.count + ' тр.' + (p.user ? ' · ' + p.user : ''),
      () => run($('minemsg'), 'Включаю «' + (p.title || 'плейлист') + '»…', { do: 'playlist', id: p.id }))));
  }
  if (likes.error) $('likesempty').textContent = likes.error;
  else {
    likeTracks = likes;
    $('likesempty').hidden = likes.length > 0; $('likesempty').textContent = 'Лайков пока нет';
    $('likes').replaceChildren(...likes.map((t, i) => trackItem(t, mmss(t.duration), () => playFrom(likeTracks, i, $('minemsg')))));
  }
}
$('likesall').addEventListener('click', () => run($('minemsg'), 'Включаю лайки…', { do: 'likes' }));

// ---- Колонка ----
function renderAbout(s) {
  const names = { playing: 'играет', paused: 'пауза', loading: 'загружает трек', idle: 'ничего не играет' };
  const rows = [
    ['Плеер', names[s.state] || s.state],
    ['Громкость', typeof s.volume === 'number' ? s.volume + ' %' : '—'],
    ['Таймер сна', s.sleep ? 'через ' + s.sleep + ' мин' : 'выключен'],
    ['SoundCloud', s.signed_in ? 'вход выполнен' : 'без входа (лайки и плейлисты недоступны)'],
    ['В очереди', s.queue.length + ' тр.'],
  ];
  const sig = JSON.stringify(rows);
  if ($('about').dataset.sig === sig) return;
  $('about').dataset.sig = sig;
  $('about').replaceChildren(...rows.map(([k, v]) => {
    const d = document.createElement('div'); const b = document.createElement('b'); b.textContent = k + ': ';
    d.append(b, v); return d;
  }));
}

// ---- tabs ----
document.querySelectorAll('nav button').forEach(b => b.addEventListener('click', () => {
  document.querySelectorAll('nav button').forEach(x => x.classList.toggle('on', x === b));
  document.querySelectorAll('.tab').forEach(t => t.classList.toggle('on', t.id === 'tab-' + b.dataset.tab));
}));

// ---- start ----
let started = false;
function start() {
  $('login').hidden = true;
  $('app').hidden = false;
  if (!started) {
    started = true;
    renderGenres();
    renderForYou();
    loadChart();
  }
  refresh();
  startPolling();
}
(async () => {
  const me = await api('/api/me');
  if (me.signed_in) start(); else showLogin();
})();
