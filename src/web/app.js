// DIOGENES — articulation area. Plain JavaScript, no build step.
// Security: the DOM is built with h() and textContent only. No innerHTML with data.
'use strict';

const API = '/api/v1/diogenes';
const LOGIN = '/entrar?next=' + encodeURIComponent('/articulacao/');
const $ = (id) => document.getElementById(id);

// ---------- DOM helpers ----------
function h(tag, attrs, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === null || v === undefined || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'style') Object.assign(el.style, v);
    else if (k === 'dataset') Object.assign(el.dataset, v);
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (v === true) el.setAttribute(k, '');
    else el.setAttribute(k, String(v));
  }
  for (const kid of kids.flat(Infinity)) {
    if (kid === null || kid === undefined || kid === false) continue;
    el.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  }
  return el;
}
function mount(...nodes) {
  const app = $('app');
  app.replaceChildren(...nodes);
}
let toastTimer;
function toast(msg) {
  const t = $('toast');
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), 3000);
}
const fmtDate = (s) => (s ? new Date(s.length === 10 ? s + 'T12:00:00' : s).toLocaleDateString('pt-BR') : '');
const fmtTime = (s) => (s ? new Date(s).toLocaleString('pt-BR') : '');
const norm = (s) => (s || '').normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

// ---------- API ----------
class ApiError extends Error {
  constructor(status, code, message) {
    super(message);
    this.status = status;
    this.code = code;
  }
}
async function api(method, path, body) {
  const opts = { method, credentials: 'same-origin', headers: { Accept: 'application/json' } };
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch(API + path, opts);
  } catch (e) {
    throw new ApiError(0, 'network', 'Sem conexão com o servidor.');
  }
  let json = null;
  try { json = await res.json(); } catch (e) { /* empty body */ }
  if (!res.ok || !json || !json.success) {
    const err = (json && json.error) || {};
    throw new ApiError(res.status, err.code || 'http_' + res.status, err.message || 'Erro ' + res.status);
  }
  return json.data;
}
function fail(err) {
  if (err && err.status === 401) {
    mount(h('div', { class: 'card stack' },
      h('h1', {}, 'Entre na sua conta'),
      h('p', {}, 'A sala de articulação é para pessoas com conta no site.'),
      h('a', { class: 'btn', href: LOGIN }, 'Entrar')));
    return;
  }
  if (err && err.code === 'module_disabled') {
    mount(h('div', { class: 'card' }, h('h1', {}, 'Recurso indisponível'),
      h('p', {}, 'A sala de articulação não está ativa nesta organização.')));
    return;
  }
  toast((err && err.message) || 'Erro inesperado.');
  console.error(err);
}

// ---------- Shared reference data ----------
const REF = { deputies: null, byId: new Map(), groups: null, groupById: new Map() };
async function loadReference() {
  if (!REF.deputies) {
    REF.deputies = await api('GET', '/deputies');
    REF.byId = new Map(REF.deputies.map((d) => [d.camara_id, d]));
  }
  if (!REF.groups) {
    REF.groups = await api('GET', '/groups');
    REF.groupById = new Map(REF.groups.map((g) => [g.id, g]));
  }
}
const camaraUrl = (id) => 'https://www.camara.leg.br/deputados/' + Number(id);
// Links and images from the upstream API: accept https only.
const safeUrl = (u) => (typeof u === 'string' && /^https:\/\/[^\s]+$/i.test(u) ? u : null);

// ---------- Router ----------
let cleanup = null;
async function route() {
  if (cleanup) { cleanup(); cleanup = null; }
  closeDrawer();
  const hash = location.hash.replace(/^#/, '') || '/';
  const [path, query] = hash.split('?');
  const params = new URLSearchParams(query || '');
  const parts = path.split('/').filter(Boolean);
  try {
    if (parts.length === 0) return await viewRooms();
    if (parts[0] === 'sala' && parts[2] === 'historico') return await viewAudit(parts[1]);
    if (parts[0] === 'sala') return await viewRoom(parts[1]);
    if (parts[0] === 'projeto') return await viewProject(parts[1], params.get('cenario'));
    if (parts[0] === 'votacoes') return await viewRollCalls();
    if (parts[0] === 'votacao') return await viewRollCall(decodeURIComponent(parts[1]));
    if (parts[0] === 'grupos') return await viewGroups();
    if (parts[0] === 'grupo') return await viewGroup(decodeURIComponent(parts[1]));
    mount(h('p', {}, 'Página não encontrada.'));
  } catch (err) {
    fail(err);
  }
}
window.addEventListener('hashchange', route);
document.addEventListener('DOMContentLoaded', route);

// ---------- Rooms ----------
async function viewRooms() {
  const data = await api('GET', '/rooms');
  $('me').textContent = data.display_name;
  const form = data.can_create_rooms
    ? h('form', {
        class: 'row',
        onsubmit: async (e) => {
          e.preventDefault();
          const name = e.target.elements.name.value;
          try {
            const room = await api('POST', '/rooms', { name });
            location.hash = '#/sala/' + room.id;
          } catch (err) { fail(err); }
        },
      },
      h('input', { name: 'name', required: true, maxlength: 120, placeholder: 'Nome da nova sala', 'aria-label': 'Nome da nova sala' }),
      h('button', { class: 'primary' }, 'Criar sala'))
    : h('p', { class: 'muted small' }, 'Para criar uma sala, peça a permissão a quem administra o site. Você também pode ser convidada(o) para uma sala existente.');
  mount(
    h('h1', {}, 'Suas salas'),
    form,
    data.rooms.length
      ? h('div', { class: 'grid', style: { marginTop: '14px' } }, data.rooms.map((r) =>
          h('a', { class: 'card', href: '#/sala/' + r.id, style: { textDecoration: 'none', color: 'inherit' } },
            h('h2', {}, r.name), h('span', { class: 'pill' }, roleLabel(r.role)))))
      : h('p', { class: 'muted' }, 'Você ainda não participa de nenhuma sala.'));
}
const roleLabel = (r) => ({ owner: 'Responsável', editor: 'Editor(a)', viewer: 'Leitor(a)' }[r] || r);

async function viewRoom(roomId) {
  const room = await api('GET', '/rooms/' + roomId);
  const isOwner = room.role === 'owner';
  const canEdit = room.role !== 'viewer';
  const members = h('ul', { class: 'list' }, room.members.map((m) =>
    h('li', {},
      h('span', { class: 'spacer' }, m.display_name),
      h('span', { class: 'pill' }, roleLabel(m.role)),
      isOwner ? h('button', {
        class: 'danger', 'aria-label': 'Remover ' + m.display_name,
        onclick: async () => {
          if (!confirm('Remover ' + m.display_name + ' da sala?')) return;
          try { await api('DELETE', `/rooms/${roomId}/members/${m.user_id}`); route(); } catch (err) { fail(err); }
        },
      }, 'Remover') : null)));
  const addMember = isOwner ? h('form', {
    class: 'row',
    onsubmit: async (e) => {
      e.preventDefault();
      const f = e.target.elements;
      try {
        await api('POST', `/rooms/${roomId}/members`, { handle: f.handle.value, role: f.role.value });
        toast('Pessoa adicionada.');
        route();
      } catch (err) { fail(err); }
    },
  },
  h('input', { name: 'handle', required: true, placeholder: '@usuario', 'aria-label': '@ da pessoa' }),
  h('select', { name: 'role', 'aria-label': 'Papel' },
    h('option', { value: 'editor' }, 'Editor(a)'),
    h('option', { value: 'viewer' }, 'Leitor(a)'),
    h('option', { value: 'owner' }, 'Responsável')),
  h('button', {}, 'Adicionar')) : null;
  const newProject = canEdit ? h('form', {
    class: 'row',
    onsubmit: async (e) => {
      e.preventDefault();
      const f = e.target.elements;
      try {
        const id = await api('POST', `/rooms/${roomId}/projects`, {
          name: f.name.value,
          threshold: Number(f.threshold.value) || 257,
          bill_id: f.bill.value ? Number(f.bill.value) : null,
        });
        location.hash = '#/projeto/' + id;
      } catch (err) { fail(err); }
    },
  },
  h('input', { name: 'name', required: true, maxlength: 120, placeholder: 'Ex.: PEC 45 — 1º turno', 'aria-label': 'Nome do projeto' }),
  h('label', {}, 'Mínimo ', h('input', { name: 'threshold', type: 'number', min: 1, max: 513, value: 257, style: { width: '80px' } })),
  h('input', { name: 'bill', type: 'number', placeholder: 'Id da proposição (opcional)', 'aria-label': 'Id da proposição na Câmara', style: { width: '210px' } }),
  h('button', { class: 'primary' }, 'Criar projeto')) : null;
  mount(
    h('div', { class: 'row' },
      h('a', { href: '#/' }, '← Salas'),
      h('span', { class: 'spacer' }),
      h('a', { href: `#/sala/${roomId}/historico` }, 'Histórico de mudanças'),
      isOwner ? h('button', { class: 'danger', onclick: async () => {
        if (!confirm('Excluir a sala e todos os projetos? Esta ação não pode ser desfeita.')) return;
        try { await api('DELETE', '/rooms/' + roomId); location.hash = '#/'; } catch (err) { fail(err); }
      } }, 'Excluir sala') : null),
    h('h1', {}, room.name),
    h('div', { class: 'grid' },
      h('section', { class: 'card stack' }, h('h2', {}, 'Projetos'),
        room.projects.length
          ? h('ul', { class: 'list' }, room.projects.map((p) =>
              h('li', {}, h('a', { href: '#/projeto/' + p.id, class: 'spacer' }, p.name),
                h('span', { class: 'pill' }, 'mínimo ' + p.threshold))))
          : h('p', { class: 'muted' }, 'Nenhum projeto ainda.'),
        newProject),
      h('section', { class: 'card stack' }, h('h2', {}, 'Pessoas'), members, addMember)));
}

async function viewAudit(roomId) {
  const rows = await api('GET', `/rooms/${roomId}/audit?limit=200`);
  const label = {
    'room.create': 'criou a sala', 'room.rename': 'renomeou a sala', 'member.set': 'definiu papel de membro',
    'member.remove': 'removeu membro', 'project.create': 'criou projeto', 'project.update': 'alterou projeto',
    'project.delete': 'excluiu projeto', 'column.create': 'criou coluna', 'column.update': 'alterou coluna',
    'column.delete': 'excluiu coluna', 'scenario.create': 'criou cenário', 'scenario.rename': 'renomeou cenário',
    'scenario.delete': 'excluiu cenário', 'position.move': 'moveu deputados', 'priority.set': 'alterou prioritários',
    'note.create': 'escreveu anotação', 'note.update': 'editou anotação', 'note.delete': 'excluiu anotação',
  };
  await loadReference().catch(() => {});
  const detail = (r) => {
    if (r.action === 'position.move') {
      const n = (r.payload.moves || []).length;
      const names = (r.payload.moves || []).slice(0, 4).map((m) => (REF.byId.get(m[0]) || {}).name || m[0]);
      return n + ' — ' + names.join(', ') + (n > 4 ? '…' : '');
    }
    return r.payload.name || '';
  };
  mount(
    h('a', { href: '#/sala/' + roomId }, '← Sala'),
    h('h1', {}, 'Histórico de mudanças'),
    h('table', {}, h('thead', {}, h('tr', {}, h('th', {}, 'Quando'), h('th', {}, 'Quem'), h('th', {}, 'O quê'), h('th', {}, 'Detalhe'))),
      h('tbody', {}, rows.map((r) => h('tr', {},
        h('td', {}, fmtTime(r.at)), h('td', {}, r.actor_name || 'ex-membro'),
        h('td', {}, label[r.action] || r.action), h('td', { class: 'small muted' }, detail(r)))))));
}

// ---------- Project board ----------
const S = {};
async function viewProject(projectId, scenarioId) {
  await loadReference();
  Object.assign(S, {
    projectId, project: null, scenario: null, selected: new Set(), lastClicked: null,
    filter: S.filterKeep || { q: '', party: '', uf: '', group: '', prio: false, diss: false },
    groupBy: S.groupByKeep || 'none',
  });
  await reloadProject(scenarioId);
  const source = new EventSource(`${API}/projects/${projectId}/events`, { withCredentials: true });
  let timer = null;
  const refresh = () => { clearTimeout(timer); timer = setTimeout(() => reloadProject(S.scenario && S.scenario.id, true), 300); };
  source.addEventListener('change', refresh);
  source.addEventListener('reload', refresh);
  cleanup = () => { source.close(); clearTimeout(timer); S.filterKeep = S.filter; S.groupByKeep = S.groupBy; };
}

async function reloadProject(scenarioId, quiet) {
  try {
    S.project = await api('GET', '/projects/' + S.projectId);
    const sid = scenarioId && S.project.scenarios.some((s) => s.id === scenarioId)
      ? scenarioId
      : (S.project.scenarios[0] && S.project.scenarios[0].id);
    S.scenario = sid ? await api('GET', '/scenarios/' + sid) : null;
    renderProject();
  } catch (err) {
    if (!quiet) throw err;
    fail(err);
  }
}

const canEdit = () => S.project && S.project.role !== 'viewer';

function positionsMap() {
  const map = new Map();
  if (S.scenario) for (const p of S.scenario.positions) map.set(p.camara_id, p.column_id);
  return map;
}
function undecidedColumn() {
  return (S.scenario ? S.scenario.columns : S.project.columns).find((c) => c.key === 'undecided');
}

function renderProject() {
  const p = S.project;
  const header = h('div', { class: 'row' },
    h('a', { href: '#/sala/' + p.room_id }, '← Sala'),
    h('h1', { style: { margin: 0 } }, p.name),
    h('span', { class: 'spacer' }),
    canEdit() ? h('button', { onclick: projectSettings }, 'Configurar') : null,
    canEdit() ? h('button', { onclick: pasteList }, 'Colar lista') : null);
  const tabs = h('div', { class: 'tabs', role: 'tablist' },
    p.scenarios.map((s) => h('button', {
      role: 'tab', 'aria-selected': S.scenario && S.scenario.id === s.id ? 'true' : 'false',
      onclick: () => { location.hash = `#/projeto/${p.id}?cenario=${s.id}`; },
    }, s.name, ' ', h('span', { class: 'pill' }, s.yes + ' sim'))),
    canEdit() ? h('button', { onclick: newScenario }, '+ Cenário') : null);
  if (!S.scenario) {
    mount(header, tabs, h('p', { class: 'muted' }, canEdit()
      ? 'Crie o primeiro cenário: em branco, ou a partir de uma votação real.'
      : 'Este projeto ainda não tem cenários.'));
    return;
  }
  mount(header, scoreboard(), tabs, scenarioTools(), filters(), board(), actionBar());
}

function scoreboard() {
  const sc = S.scenario;
  const c = sc.counts;
  const total = sc.total || 513;
  const pct = (n) => (100 * n) / total + '%';
  const seg = (cls, n, label) => h('div', { class: cls, style: { width: pct(n) }, title: `${label}: ${n}` }, n / total > 0.04 ? String(n) : '');
  const o = sc.outcome;
  const text = o.state === 'approved'
    ? `APROVADO — ${c.yes} votos Sim (${o.margin} acima do mínimo de ${sc.threshold})`
    : o.state === 'rejected'
      ? `REJEITADO — com ${c.no} votos Não, o Sim não chega mais a ${sc.threshold}`
      : `Faltam ${o.missing} votos Sim para aprovar · Sim + Em disputa = ${c.yes + c.disputed} (${o.reachable ? 'alcança' : 'não alcança'} o mínimo)`;
  const prio = new Set(S.project.priorities);
  const pos = positionsMap();
  const prioInCol = (key) => {
    const col = sc.columns.find((x) => x.key === key);
    if (!col) return 0;
    let n = 0;
    for (const id of prio) if ((pos.get(id) || undecidedColumn().id) === col.id) n++;
    return n;
  };
  const tile = (cls, label, n, key) => h('div', { class: 'tile ' + cls },
    h('span', { class: 'small muted' }, label), h('b', {}, n),
    prio.size ? h('span', { class: 'small muted' }, '★ ' + prioInCol(key)) : null);
  const custom = sc.columns.filter((x) => !x.key);
  return h('section', { class: 'score', 'aria-label': 'Placar' },
    h('div', { class: 'tiles' },
      tile('yes', 'Sim (mínimo ' + sc.threshold + ')', c.yes, 'yes'),
      tile('no', 'Não', c.no, 'no'),
      tile('disputed', 'Em disputa', c.disputed, 'disputed'),
      tile('', 'Sem posição', sc.column_counts[undecidedColumn().id] || 0, 'undecided'),
      tile('', 'Ausentes', c.absent, 'absent'),
      tile('', 'Presidente', c.chair, 'chair')),
    h('div', { class: 'bar', role: 'img', 'aria-label': `Sim ${c.yes}, Ausente ${c.absent}, Presidente ${c.chair}, Não ${c.no}, de ${total}` },
      seg('yes', c.yes, 'Sim'), h('div', { class: 'gap' }), seg('absent', c.absent, 'Ausente'),
      seg('chair', c.chair, 'Presidente'), seg('no', c.no, 'Não'),
      h('div', { class: 'marker', style: { left: pct(sc.threshold) }, title: 'Mínimo: ' + sc.threshold })),
    h('div', { class: 'outcome ' + o.state }, text),
    custom.length ? h('p', { class: 'small muted' }, 'Colunas próprias: ' + custom.map((x) => `${x.name} ${sc.column_counts[x.id] || 0}`).join(' · ')) : null,
    sc.orientations.length ? h('p', { class: 'small muted' },
      'Orientações: ' + sc.orientations.filter((o2) => o2[1]).map((o2) => o2[0] + ' ' + o2[1]).join(' · ') +
      (sc.dissidents.length ? ` · ${sc.dissidents.length} dissidentes` : '')) : null);
}

function scenarioTools() {
  if (!canEdit()) return null;
  const sc = S.scenario;
  return h('div', { class: 'row small' },
    sc.source_roll_call ? h('span', { class: 'muted' }, 'Origem: votação ', h('a', { href: '#/votacao/' + encodeURIComponent(sc.source_roll_call) }, sc.source_roll_call)) : null,
    h('span', { class: 'spacer' }),
    h('button', { onclick: async () => {
      const name = prompt('Novo nome do cenário:', sc.name);
      if (!name) return;
      try { await api('PATCH', '/scenarios/' + sc.id, { name }); await reloadProject(sc.id); } catch (err) { fail(err); }
    } }, 'Renomear cenário'),
    h('button', { onclick: addColumn }, '+ Coluna'),
    h('button', { class: 'danger', onclick: async () => {
      if (!confirm('Excluir o cenário "' + sc.name + '"?')) return;
      try { await api('DELETE', '/scenarios/' + sc.id); await reloadProject(null); } catch (err) { fail(err); }
    } }, 'Excluir cenário'));
}

function filters() {
  const f = S.filter;
  const parties = [...new Set(REF.deputies.map((d) => d.party))].sort();
  const ufs = [...new Set(REF.deputies.map((d) => d.uf))].sort();
  const set = (k, v) => { f[k] = v; renderProject(); };
  const groups = REF.groups.filter((g) => g.in_office_count > 0);
  return h('div', { class: 'filters' },
    h('input', { type: 'search', placeholder: 'Buscar nome, partido ou UF', value: f.q, 'aria-label': 'Buscar',
      oninput: (e) => { f.q = e.target.value; clearTimeout(S.qTimer); S.qTimer = setTimeout(() => { renderProject(); const el = document.querySelector('.filters input[type=search]'); if (el) { el.focus(); el.setSelectionRange(el.value.length, el.value.length); } }, 200); } }),
    h('select', { 'aria-label': 'Partido', onchange: (e) => set('party', e.target.value) },
      h('option', { value: '' }, 'Todos os partidos'), parties.map((x) => h('option', { value: x, selected: f.party === x }, x))),
    h('select', { 'aria-label': 'UF', onchange: (e) => set('uf', e.target.value) },
      h('option', { value: '' }, 'Todas as UFs'), ufs.map((x) => h('option', { value: x, selected: f.uf === x }, x))),
    h('select', { 'aria-label': 'Frente ou bancada', onchange: (e) => set('group', e.target.value), style: { maxWidth: '280px' } },
      h('option', { value: '' }, 'Todas as frentes e bancadas'),
      h('optgroup', { label: 'Bancadas' }, groups.filter((g) => g.kind === 'caucus').map((g) => h('option', { value: g.id, selected: f.group === g.id }, `${g.title} (${g.in_office_count})`))),
      h('optgroup', { label: 'Frentes' }, groups.filter((g) => g.kind === 'front').map((g) => h('option', { value: g.id, selected: f.group === g.id }, `${g.title} (${g.in_office_count})`)))),
    h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: f.prio, onchange: (e) => set('prio', e.target.checked) }), 'Só prioritários'),
    S.scenario.dissidents.length ? h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: f.diss, onchange: (e) => set('diss', e.target.checked) }), 'Só dissidentes') : null,
    h('select', { 'aria-label': 'Agrupar', onchange: (e) => { S.groupBy = e.target.value; renderProject(); } },
      h('option', { value: 'none', selected: S.groupBy === 'none' }, 'Sem agrupar'),
      h('option', { value: 'party', selected: S.groupBy === 'party' }, 'Agrupar por partido'),
      h('option', { value: 'uf', selected: S.groupBy === 'uf' }, 'Agrupar por UF')));
}

function visibleDeputies() {
  const f = S.filter;
  const q = norm(f.q);
  const prio = new Set(S.project.priorities);
  const diss = new Set(S.scenario.dissidents);
  return REF.deputies.filter((d) =>
    (!q || norm(`${d.name} ${d.party} ${d.uf}`).includes(q)) &&
    (!f.party || d.party === f.party) &&
    (!f.uf || d.uf === f.uf) &&
    (!f.group || d.groups.includes(f.group)) &&
    (!f.prio || prio.has(d.camara_id)) &&
    (!f.diss || diss.has(d.camara_id)));
}

function board() {
  const sc = S.scenario;
  const pos = positionsMap();
  const und = undecidedColumn();
  const prio = new Set(S.project.priorities);
  const diss = new Set(sc.dissidents);
  const visible = visibleDeputies();
  const byCol = new Map(sc.columns.map((c) => [c.id, []]));
  for (const d of visible) {
    const col = pos.get(d.camara_id) || und.id;
    (byCol.get(col) || byCol.get(und.id)).push(d);
  }
  const order = visible.map((d) => d.camara_id);
  return h('div', { class: 'board' }, sc.columns.map((col) => {
    const deps = byCol.get(col.id) || [];
    const groups = new Map();
    if (S.groupBy === 'none') groups.set('', deps);
    else for (const d of deps) {
      const k = S.groupBy === 'party' ? d.party : d.uf;
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k).push(d);
    }
    const body = h('div', { class: 'col-body' });
    for (const [k, list] of [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
      if (k) body.append(h('div', {
        class: 'group-head', draggable: canEdit() ? 'true' : null,
        ondragstart: (e) => startDrag(e, list.map((d) => d.camara_id)),
      }, `${k} · ${list.length}`));
      for (const d of list) body.append(card(d, order, prio.has(d.camara_id), diss.has(d.camara_id)));
    }
    const column = h('section', { class: 'col', 'aria-label': col.name },
      h('div', { class: 'col-head' },
        h('span', { class: 'dot', style: { background: col.color } }),
        h('b', {}, col.name),
        h('span', { class: 'pill', title: 'Total no cenário' }, String(sc.column_counts[col.id] || 0) + (deps.length !== (sc.column_counts[col.id] || 0) ? ` (${deps.length} no filtro)` : '')),
        canEdit() ? h('button', { class: 'link small', title: 'Selecionar todos desta coluna no filtro', onclick: () => { deps.forEach((d) => S.selected.add(d.camara_id)); renderProject(); } }, 'Selecionar') : null,
        canEdit() && !col.key ? h('button', { class: 'link small', title: 'Renomear ou excluir coluna', onclick: () => editColumn(col) }, '⋯') : null),
      body);
    if (canEdit()) {
      column.addEventListener('dragover', (e) => { e.preventDefault(); column.classList.add('over'); });
      column.addEventListener('dragleave', () => column.classList.remove('over'));
      column.addEventListener('drop', (e) => {
        e.preventDefault();
        column.classList.remove('over');
        let ids = [];
        try { ids = JSON.parse(e.dataTransfer.getData('text/plain')); } catch (err) { return; }
        if (Array.isArray(ids) && ids.length) move(ids, col.id);
      });
    }
    return column;
  }));
}

function card(d, order, isPrio, isDiss) {
  const sel = S.selected.has(d.camara_id);
  const el = h('div', {
    class: 'dep' + (sel ? ' sel' : ''), tabindex: '0', role: 'button',
    'aria-pressed': sel ? 'true' : 'false', draggable: canEdit() ? 'true' : null,
    title: 'Clique para selecionar; duplo clique abre a ficha',
    ondragstart: (e) => startDrag(e, S.selected.has(d.camara_id) ? [...S.selected] : [d.camara_id]),
    onclick: (e) => toggleSelect(d.camara_id, order, e.shiftKey),
    ondblclick: () => openDeputy(d.camara_id),
    onkeydown: (e) => {
      if (e.key === ' ') { e.preventDefault(); toggleSelect(d.camara_id, order, e.shiftKey); }
      if (e.key === 'Enter') { e.preventDefault(); openDeputy(d.camara_id); }
    },
  },
  isPrio ? h('span', { class: 'star', title: 'Prioritário' }, '★') : null,
  h('span', { class: 'name' }, d.name),
  d.board_role === 'Presidente' ? h('span', { class: 'tag' }, 'PRES') : null,
  d.leader_role ? h('span', { class: 'tag', title: d.leader_role }, 'LÍD') : null,
  isDiss ? h('span', { class: 'diss', title: 'Votou contra a orientação' }, 'DISS') : null,
  h('span', { class: 'meta' }, `${d.party}/${d.uf}`));
  return el;
}

function toggleSelect(id, order, range) {
  if (!canEdit()) { openDeputy(id); return; }
  if (range && S.lastClicked !== null) {
    const a = order.indexOf(S.lastClicked);
    const b = order.indexOf(id);
    if (a >= 0 && b >= 0) for (const x of order.slice(Math.min(a, b), Math.max(a, b) + 1)) S.selected.add(x);
  } else if (S.selected.has(id)) S.selected.delete(id);
  else S.selected.add(id);
  S.lastClicked = id;
  renderProject();
}

function startDrag(e, ids) {
  e.dataTransfer.setData('text/plain', JSON.stringify(ids));
  e.dataTransfer.effectAllowed = 'move';
  const ghost = h('div', { class: 'pill', style: { position: 'absolute', top: '-100px' } }, ids.length === 1 ? '1 deputado' : `${ids.length} deputados`);
  document.body.append(ghost);
  e.dataTransfer.setDragImage(ghost, 10, 10);
  setTimeout(() => ghost.remove(), 0);
}

function actionBar() {
  if (!canEdit() || S.selected.size === 0) return null;
  const select = h('select', { 'aria-label': 'Mover para' }, S.scenario.columns.map((c) => h('option', { value: c.id }, c.name)));
  return h('div', { class: 'actionbar', role: 'region', 'aria-label': 'Ações da seleção' },
    h('b', {}, `${S.selected.size} selecionado(s)`),
    h('span', {}, 'Mover para'), select,
    h('button', { class: 'primary', onclick: () => move([...S.selected], select.value) }, 'Mover'),
    h('button', { onclick: () => setPriority([...S.selected], true) }, '★ Prioritários'),
    h('button', { onclick: () => setPriority([...S.selected], false) }, 'Tirar ★'),
    h('button', { onclick: () => { S.selected.clear(); renderProject(); } }, 'Limpar seleção'));
}

async function move(ids, columnId) {
  const sc = S.scenario;
  const und = undecidedColumn();
  // Optimistic update.
  const keep = sc.positions.filter((p) => !ids.includes(p.camara_id));
  if (columnId !== und.id) for (const id of ids) keep.push({ camara_id: id, column_id: columnId });
  sc.positions = keep;
  S.selected.clear();
  renderProject();
  try {
    await api('PUT', `/scenarios/${sc.id}/positions`, {
      moves: ids.map((id) => ({ camara_id: id, column_id: columnId === und.id ? null : columnId })),
    });
  } catch (err) { fail(err); }
  await reloadProject(sc.id, true);
}

async function setPriority(ids, value) {
  try {
    await api('PUT', `/projects/${S.projectId}/priorities`, { camara_ids: ids, value });
    S.selected.clear();
    await reloadProject(S.scenario && S.scenario.id, true);
  } catch (err) { fail(err); }
}

async function addColumn() {
  const name = prompt('Nome da nova coluna (ex.: "Indecisos PSD"):');
  if (!name) return;
  try { await api('POST', `/projects/${S.projectId}/columns`, { name }); await reloadProject(S.scenario.id); } catch (err) { fail(err); }
}

async function editColumn(col) {
  const name = prompt('Novo nome da coluna (deixe em branco para EXCLUIR):', col.name);
  if (name === null) return;
  try {
    if (name.trim() === '') {
      if (!confirm(`Excluir a coluna "${col.name}"? Os deputados nela voltam para "Sem posição".`)) return;
      await api('DELETE', '/columns/' + col.id);
    } else {
      await api('PATCH', '/columns/' + col.id, { name });
    }
    await reloadProject(S.scenario.id);
  } catch (err) { fail(err); }
}

async function newScenario() {
  let rollCalls = [];
  try {
    const q = S.project.bill_id ? `?limit=100&bill=${S.project.bill_id}` : '?limit=100';
    rollCalls = await api('GET', '/roll-calls' + q);
  } catch (err) { fail(err); }
  const form = h('form', { class: 'stack', onsubmit: async (e) => {
    e.preventDefault();
    const f = e.target.elements;
    const body = { name: f.name.value };
    if (f.mode.value === 'copy' && S.scenario) body.copy_from = S.scenario.id;
    if (f.mode.value === 'roll') { body.roll_call = f.roll.value; body.absent_if_missing = f.absent.checked; }
    try {
      const id = await api('POST', `/projects/${S.projectId}/scenarios`, body);
      closeDrawer();
      location.hash = `#/projeto/${S.projectId}?cenario=${id}`;
    } catch (err) { fail(err); }
  } },
  h('h2', {}, 'Novo cenário'),
  h('label', {}, 'Nome ', h('input', { name: 'name', required: true, maxlength: 120, value: 'Cenário ' + (S.project.scenarios.length + 1) })),
  h('fieldset', { class: 'stack' }, h('legend', {}, 'Começar de'),
    h('label', { class: 'check' }, h('input', { type: 'radio', name: 'mode', value: 'blank', checked: true }), 'Em branco (Presidente já posicionado)'),
    S.scenario ? h('label', { class: 'check' }, h('input', { type: 'radio', name: 'mode', value: 'copy' }), `Cópia de "${S.scenario.name}"`) : null,
    h('label', { class: 'check' }, h('input', { type: 'radio', name: 'mode', value: 'roll', disabled: !rollCalls.length }), 'Uma votação nominal real')),
  h('select', { name: 'roll', 'aria-label': 'Votação', style: { width: '100%' } }, rollCalls.map((r) =>
    h('option', { value: r.id }, `${fmtDate(r.voted_on)} · ${r.bill_label || r.id} · ${(r.description || '').slice(0, 90)}`))),
  h('label', { class: 'check' }, h('input', { type: 'checkbox', name: 'absent', checked: true }), 'Quem não votou fica em "Ausente"'),
  !rollCalls.length ? h('p', { class: 'small muted' }, 'Nenhuma votação nominal sincronizada ainda' + (S.project.bill_id ? ' para esta proposição.' : '.')) : null,
  h('div', { class: 'row' }, h('button', { class: 'primary' }, 'Criar'), h('button', { type: 'button', onclick: closeDrawer }, 'Cancelar')));
  openDrawer(form);
}

function projectSettings() {
  const p = S.project;
  openDrawer(h('form', { class: 'stack', onsubmit: async (e) => {
    e.preventDefault();
    const f = e.target.elements;
    try {
      await api('PATCH', '/projects/' + p.id, {
        name: f.name.value, threshold: Number(f.threshold.value),
        bill_id: f.bill.value ? Number(f.bill.value) : null,
      });
      closeDrawer();
      await reloadProject(S.scenario && S.scenario.id);
    } catch (err) { fail(err); }
  } },
  h('h2', {}, 'Configurar projeto'),
  h('label', {}, 'Nome ', h('input', { name: 'name', required: true, maxlength: 120, value: p.name })),
  h('label', {}, 'Mínimo para aprovar ', h('input', { name: 'threshold', type: 'number', min: 1, max: 513, value: p.threshold })),
  h('p', { class: 'small muted' }, '257 = maioria absoluta · 308 = 3/5 (PEC)'),
  h('label', {}, 'Id da proposição na Câmara ', h('input', { name: 'bill', type: 'number', value: p.bill_id || '' })),
  h('div', { class: 'row' }, h('button', { class: 'primary' }, 'Salvar'), h('button', { type: 'button', onclick: closeDrawer }, 'Cancelar'),
    p.role === 'owner' ? h('button', { type: 'button', class: 'danger', onclick: async () => {
      if (!confirm('Excluir o projeto e todos os cenários?')) return;
      try { await api('DELETE', '/projects/' + p.id); closeDrawer(); location.hash = '#/sala/' + p.room_id; } catch (err) { fail(err); }
    } }, 'Excluir projeto') : null)));
}

function pasteList() {
  const out = h('div', { class: 'stack' });
  openDrawer(h('div', { class: 'stack' },
    h('h2', {}, 'Colar lista de nomes'),
    h('p', { class: 'small muted' }, 'Um nome por linha, ou separados por vírgula. Serve para marcar prioritários ou selecionar deputados.'),
    h('textarea', { id: 'paste', rows: 10 }),
    h('button', { class: 'primary', onclick: async () => {
      try {
        const res = await api('POST', `/projects/${S.projectId}/match-list`, { text: $('paste').value });
        out.replaceChildren(
          h('p', {}, `${res.found.length} encontrados · ${res.missed.length} sem correspondência`),
          res.missed.length ? h('p', { class: 'small muted' }, 'Sem correspondência: ' + res.missed.join('; ')) : null,
          h('div', { class: 'row' },
            h('button', { onclick: async () => { await setPriority(res.found.map((x) => x[0]), true); closeDrawer(); toast('Prioritários marcados.'); } }, '★ Marcar como prioritários'),
            h('button', { onclick: () => { res.found.forEach((x) => S.selected.add(x[0])); closeDrawer(); renderProject(); } }, 'Selecionar no quadro')));
      } catch (err) { fail(err); }
    } }, 'Conferir nomes'),
    out));
}

// ---------- Deputy drawer ----------
function openDrawer(content) {
  const d = $('drawer');
  d.replaceChildren(h('button', { class: 'link', onclick: closeDrawer, style: { float: 'right' } }, 'Fechar ✕'), content);
  d.classList.add('open');
  d.setAttribute('aria-hidden', 'false');
}
function closeDrawer() {
  const d = $('drawer');
  if (!d) return;
  d.classList.remove('open');
  d.setAttribute('aria-hidden', 'true');
}
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeDrawer(); });

function openDeputy(id) {
  const d = REF.byId.get(id);
  if (!d) return;
  const pos = positionsMap();
  const sc = S.scenario;
  const current = pos.get(id) || undecidedColumn().id;
  const isPrio = S.project.priorities.includes(id);
  const notes = S.project.notes.filter((n) => n.camara_id === id);
  const groups = d.groups.map((g) => REF.groupById.get(g)).filter(Boolean);
  openDrawer(h('div', { class: 'stack' },
    h('div', { class: 'row' },
      safeUrl(d.photo_url) ? h('img', { src: safeUrl(d.photo_url), alt: '', loading: 'lazy', referrerpolicy: 'no-referrer' }) : null,
      h('div', {}, h('h2', {}, d.name), h('div', { class: 'muted' }, `${d.party} / ${d.uf}`),
        d.board_role ? h('div', { class: 'small' }, 'Mesa: ' + d.board_role) : null,
        d.leader_role ? h('div', { class: 'small' }, 'Liderança: ' + d.leader_role) : null,
        h('a', { href: camaraUrl(id), target: '_blank', rel: 'noopener' }, 'Página na Câmara ↗'))),
    canEdit() ? h('label', {}, 'Posição neste cenário ',
      h('select', { onchange: (e) => { move([id], e.target.value); closeDrawer(); } },
        sc.columns.map((c) => h('option', { value: c.id, selected: c.id === current }, c.name)))) : null,
    canEdit() ? h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: isPrio, onchange: (e) => setPriority([id], e.target.checked) }), 'Prioritário neste projeto') : null,
    h('h3', {}, 'Anotações'),
    notes.length ? notes.map((n) => h('div', { class: 'note' + (n.visibility === 'author' ? ' private' : '') },
      h('div', { class: 'small muted' }, `${n.author_name || 'ex-membro'} · ${fmtTime(n.updated_at)}${n.visibility === 'author' ? ' · só você vê' : ''}`),
      h('div', { style: { whiteSpace: 'pre-wrap' } }, n.body),
      h('button', { class: 'link small', onclick: async () => {
        if (!confirm('Excluir esta anotação?')) return;
        try { await api('DELETE', '/notes/' + n.id); await reloadProject(sc.id); openDeputy(id); } catch (err) { fail(err); }
      } }, 'Excluir'))) : h('p', { class: 'muted small' }, 'Nenhuma anotação.'),
    canEdit() ? h('form', { class: 'stack', onsubmit: async (e) => {
      e.preventDefault();
      const f = e.target.elements;
      try {
        await api('POST', `/projects/${S.projectId}/notes`, { camara_id: id, body: f.body.value, private: f.private.checked });
        await reloadProject(sc.id);
        openDeputy(id);
      } catch (err) { fail(err); }
    } },
    h('textarea', { name: 'body', required: true, maxlength: 10000, placeholder: 'Nova anotação', 'aria-label': 'Nova anotação' }),
    h('label', { class: 'check small' }, h('input', { type: 'checkbox', name: 'private' }), 'Só eu posso ver'),
    h('button', { class: 'primary' }, 'Salvar anotação')) : null,
    h('details', {}, h('summary', {}, `Frentes e bancadas (${groups.length})`),
      groups.length ? h('ul', { class: 'list small' }, groups.map((g) => h('li', {}, h('a', { href: '#/grupo/' + encodeURIComponent(g.id) }, g.title)))) : h('p', { class: 'muted small' }, 'Nenhuma registrada.')),
    h('p', { class: 'small muted' }, 'Anotações são sobre a atuação pública do parlamentar. Não registre dados pessoais sensíveis.')));
}

// ---------- Public data pages ----------
async function viewRollCalls() {
  const rows = await api('GET', '/roll-calls?limit=200');
  let status = [];
  try { status = await api('GET', '/sync/status'); } catch (err) { /* optional */ }
  mount(
    h('h1', {}, 'Votações nominais do Plenário'),
    status.length ? h('p', { class: 'small muted' }, 'Dados da Câmara dos Deputados e do TSE. Última sincronização: ' +
      status.map((s) => `${s.job} ${fmtTime(s.finished_at || s.started_at)} (${s.status})`).join(' · ')) : null,
    h('table', {}, h('thead', {}, h('tr', {}, h('th', {}, 'Data'), h('th', {}, 'Proposição'), h('th', {}, 'Descrição'), h('th', {}, 'Votos'))),
      h('tbody', {}, rows.map((r) => h('tr', {},
        h('td', {}, fmtDate(r.voted_on)), h('td', {}, r.bill_label || '—'),
        h('td', {}, h('a', { href: '#/votacao/' + encodeURIComponent(r.id) }, r.description || r.id)),
        h('td', {}, String(r.vote_count), r.totals_match === false ? h('span', { class: 'pill', title: 'Os votos não batem com o total publicado' }, ' ≠ total') : null))))));
}

async function viewRollCall(id) {
  const rc = await api('GET', '/roll-calls/' + encodeURIComponent(id));
  const diss = new Set(rc.dissidents);
  let q = '';
  const tbody = h('tbody');
  const draw = () => tbody.replaceChildren(...rc.votes
    .filter((v) => !q || norm(`${v.name} ${v.party} ${v.uf} ${v.vote}`).includes(q))
    .map((v) => h('tr', {}, h('td', {}, v.name), h('td', {}, `${v.party}/${v.uf}`), h('td', {}, v.vote),
      h('td', {}, diss.has(v.camara_id) ? 'contra a orientação' : ''))));
  draw();
  mount(
    h('a', { href: '#/votacoes' }, '← Votações'),
    h('h1', {}, rc.bill_label || rc.id),
    h('p', {}, rc.description),
    rc.bill_summary ? h('p', { class: 'small muted' }, rc.bill_summary) : null,
    h('p', {}, Object.entries(rc.counts).map(([k, n]) => `${k}: ${n}`).join(' · '),
      rc.totals_match === true ? h('span', { class: 'pill' }, ' confere com o total publicado') : null),
    rc.orientations.length ? h('p', { class: 'small' }, 'Orientações: ' + rc.orientations.filter((o) => o[1]).map((o) => o[0] + ' ' + o[1]).join(' · ')) : null,
    h('p', {}, `${rc.dissidents.length} votos contra a orientação do partido ou bloco.`),
    h('input', { type: 'search', placeholder: 'Filtrar', 'aria-label': 'Filtrar votos', oninput: (e) => { q = norm(e.target.value); draw(); } }),
    h('table', {}, h('thead', {}, h('tr', {}, h('th', {}, 'Deputado'), h('th', {}, 'Partido/UF'), h('th', {}, 'Voto'), h('th', {}, ''))), tbody));
}

async function viewGroups() {
  const groups = await api('GET', '/groups');
  const section = (kind, title) => h('section', { class: 'stack' }, h('h2', {}, title),
    h('table', {}, h('thead', {}, h('tr', {}, h('th', {}, 'Nome'), h('th', {}, 'Em exercício'), h('th', {}, 'Assinaturas na criação'), h('th', {}, 'Fonte'))),
      h('tbody', {}, groups.filter((g) => g.kind === kind).map((g) => h('tr', {},
        h('td', {}, h('a', { href: '#/grupo/' + encodeURIComponent(g.id) }, g.title)),
        h('td', {}, String(g.in_office_count)),
        h('td', {}, g.official_total === null ? '—' : String(g.official_total)),
        h('td', {}, g.source === 'tse' ? 'TSE (autodeclaração)' : 'Câmara'))))));
  mount(h('h1', {}, 'Frentes e bancadas'), section('caucus', 'Bancadas'), section('front', 'Frentes parlamentares'));
}

async function viewGroup(id) {
  const g = await api('GET', '/groups/' + encodeURIComponent(id));
  mount(
    h('a', { href: '#/grupos' }, '← Frentes e bancadas'),
    h('h1', {}, g.title),
    h('p', { class: 'muted' }, `${g.in_office_count} em exercício · ${g.member_count} registrados` +
      (g.official_total !== null ? ` · ${g.official_total} assinaturas na criação` : '') +
      (g.details && g.details.signatures_net ? ` · saldo de adesões e retiradas: ${g.details.signatures_net}` : '') +
      (g.source === 'tse' ? ' · fonte: autodeclaração de cor/raça ao TSE (2022)' : ' · fonte: Câmara dos Deputados')),
    g.details && safeUrl(g.details.document_url) ? h('p', {}, h('a', { href: safeUrl(g.details.document_url), target: '_blank', rel: 'noopener' }, 'Documento oficial ↗')) : null,
    h('table', {}, h('thead', {}, h('tr', {}, h('th', {}, 'Deputado'), h('th', {}, 'Papel'), h('th', {}, 'Situação'))),
      h('tbody', {}, g.members.map((m) => h('tr', {},
        h('td', {}, h('a', { href: camaraUrl(m.camara_id), target: '_blank', rel: 'noopener' }, m.name || ('#' + m.camara_id))),
        h('td', {}, m.role || ''), h('td', {}, m.in_office ? 'em exercício' : 'fora de exercício'))))));
}
