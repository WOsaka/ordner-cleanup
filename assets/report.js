(() => {
  'use strict';

  const data = JSON.parse(document.getElementById('report-data').textContent);
  const nf = new Intl.NumberFormat('de-DE');
  const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];

  function bytes(n) {
    if (n < 1024) return nf.format(n) + ' B';
    let v = n;
    let i = 0;
    while (v >= 1024 && i < UNITS.length - 1) { v /= 1024; i++; }
    const digits = v < 10 ? 2 : v < 100 ? 1 : 0;
    return new Intl.NumberFormat('de-DE', { maximumFractionDigits: digits }).format(v) + ' ' + UNITS[i];
  }
  const date = (s) => new Date(s * 1000).toLocaleDateString('de-DE');
  const pct = (x) => new Intl.NumberFormat('de-DE', { maximumFractionDigits: x < 0.1 ? 1 : 0 }).format(x * 100) + ' %';
  const plural = (n, one, many) => nf.format(n) + ' ' + (n === 1 ? one : many);

  // Alle Texte gehen über textContent bzw. Textknoten: Dateinamen werden nie als HTML gelesen.
  function el(tag, attrs, ...kids) {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (k === 'class') e.className = v;
      else if (k === 'text') e.textContent = v;
      else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
      else if (v !== false && v != null) e.setAttribute(k, v);
    }
    for (const kid of kids.flat(Infinity)) if (kid != null) e.append(kid);
    return e;
  }

  const slot = (id) => document.querySelector('[data-slot="' + id + '"]');

  /* ---------- Tabellen ---------- */

  function dataTable(columns, rows, opts) {
    const o = Object.assign({ pageSize: 100, empty: 'Keine Einträge.', facet: null }, opts);
    const wrap = el('div', { class: 'table-wrap' });
    if (!rows.length) {
      wrap.append(el('p', { class: 'empty', text: o.empty }));
      return wrap;
    }
    const texts = rows.map((r) =>
      columns.map((c) => String(c.text ? c.text(r) : c.get(r))).join(' ').toLowerCase());
    let sortIndex = -1;
    let sortDir = 1;
    let query = '';
    let facetValue = '';
    let shown = o.pageSize;

    const tools = el('div', { class: 'tools' });
    const search = el('input', {
      type: 'search', placeholder: 'Filtern …', 'aria-label': 'Tabelle filtern',
      oninput: (e) => { query = e.target.value.trim().toLowerCase(); shown = o.pageSize; draw(); },
    });
    tools.append(search);
    if (o.facet) {
      const counts = new Map();
      for (const r of rows) for (const v of o.facet.get(r)) counts.set(v, (counts.get(v) || 0) + 1);
      const select = el('select', {
        'aria-label': o.facet.label,
        onchange: (e) => { facetValue = e.target.value; shown = o.pageSize; draw(); },
      }, el('option', { value: '', text: o.facet.label + ': alle' }));
      for (const [v, n] of [...counts].sort()) {
        select.append(el('option', { value: v, text: v + ' (' + nf.format(n) + ')' }));
      }
      tools.append(select);
    }
    const count = el('span', { class: 'count', 'aria-live': 'polite' });
    tools.append(count);

    const head = el('tr');
    const ths = columns.map((c, i) => {
      const th = el('th', { class: c.num ? 'num' : '', scope: 'col' },
        el('button', {
          type: 'button', text: c.label,
          onclick: () => {
            sortDir = sortIndex === i ? -sortDir : (c.num ? -1 : 1);
            sortIndex = i;
            draw();
          },
        }));
      head.append(th);
      return th;
    });
    const body = el('tbody');
    const more = el('button', { type: 'button', class: 'more' });
    more.addEventListener('click', () => { shown += o.pageSize; draw(); });
    wrap.append(tools, el('div', { class: 'scroll' }, el('table', null, el('thead', null, head), body)), more);

    function draw() {
      let idx = rows.map((_, i) => i);
      if (query) idx = idx.filter((i) => texts[i].includes(query));
      if (facetValue) idx = idx.filter((i) => o.facet.get(rows[i]).includes(facetValue));
      if (sortIndex >= 0) {
        const c = columns[sortIndex];
        const key = (i) => (c.sort ? c.sort(rows[i]) : c.get(rows[i]));
        idx.sort((a, b) => {
          const x = key(a); const y = key(b);
          const r = typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y), 'de');
          return r * sortDir || a - b;
        });
      }
      ths.forEach((th, i) => th.setAttribute('aria-sort',
        i === sortIndex ? (sortDir === 1 ? 'ascending' : 'descending') : 'none'));
      body.replaceChildren();
      for (const i of idx.slice(0, shown)) {
        const tr = el('tr');
        columns.forEach((c) => {
          const v = c.render ? c.render(rows[i]) : c.get(rows[i]);
          tr.append(el('td', { class: (c.num ? 'num ' : '') + (c.cls || '') }, v));
        });
        body.append(tr);
      }
      count.textContent = idx.length === rows.length
        ? plural(rows.length, 'Eintrag', 'Einträge')
        : nf.format(idx.length) + ' von ' + plural(rows.length, 'Eintrag', 'Einträgen');
      more.hidden = idx.length <= shown;
      more.textContent = 'Weitere ' + nf.format(Math.min(o.pageSize, idx.length - shown)) + ' anzeigen';
      if (!idx.length) body.append(el('tr', null, el('td', { colspan: columns.length, class: 'empty', text: 'Nichts gefunden.' })));
    }
    draw();
    return wrap;
  }

  function badges(f) {
    return [
      f.hidden ? el('span', { class: 'badge', text: 'versteckt' }) : null,
      f.system ? el('span', { class: 'badge', text: 'System' }) : null,
      f.cloud_only ? el('span', { class: 'badge', text: 'nur Cloud' }) : null,
    ];
  }

  const fileColumns = [
    { label: 'Pfad', get: (f) => f.path, cls: 'path', render: (f) => [f.path, badges(f)] },
    { label: 'Größe', num: true, get: (f) => f.size, render: (f) => bytes(f.size) },
    { label: 'Geändert', num: true, get: (f) => f.mtime, render: (f) => date(f.mtime) },
    { label: 'Alter (Tage)', num: true, get: (f) => f.age_days, render: (f) => nf.format(f.age_days) },
  ];

  /* ---------- Gruppenlisten (Duplikate, ähnliche Dateien) ---------- */

  function groupList(items, describe, opts) {
    const o = Object.assign({ pageSize: 50, empty: 'Keine Einträge.' }, opts);
    const wrap = el('div', { class: 'table-wrap' });
    if (!items.length) { wrap.append(el('p', { class: 'empty', text: o.empty })); return wrap; }
    const texts = items.map((it) => describe(it).search.toLowerCase());
    let query = '';
    let shown = o.pageSize;
    const count = el('span', { class: 'count', 'aria-live': 'polite' });
    const search = el('input', {
      type: 'search', placeholder: 'Filtern …', 'aria-label': 'Gruppen filtern',
      oninput: (e) => { query = e.target.value.trim().toLowerCase(); shown = o.pageSize; draw(); },
    });
    const list = el('div', { class: 'groups' });
    const more = el('button', { type: 'button', class: 'more' });
    more.addEventListener('click', () => { shown += o.pageSize; draw(); });
    wrap.append(el('div', { class: 'tools' }, search, count), list, more);
    function draw() {
      const idx = items.map((_, i) => i).filter((i) => !query || texts[i].includes(query));
      list.replaceChildren();
      for (const i of idx.slice(0, shown)) {
        const d = describe(items[i]);
        list.append(el('details', null,
          el('summary', null, el('span', { class: 'title', text: d.title }), el('span', { class: 'meta', text: d.meta }), d.badge || null),
          el('ul', null, d.lines.map((l) => el('li', null, l)))));
      }
      count.textContent = idx.length === items.length
        ? plural(items.length, 'Gruppe', 'Gruppen')
        : nf.format(idx.length) + ' von ' + plural(items.length, 'Gruppe', 'Gruppen');
      more.hidden = idx.length <= shown;
      more.textContent = 'Weitere ' + nf.format(Math.min(o.pageSize, idx.length - shown)) + ' anzeigen';
    }
    draw();
    return wrap;
  }

  /* ---------- Treemap ---------- */

  function worst(row, side) {
    const s = row.reduce((a, b) => a + b, 0);
    return Math.max((side * side * Math.max(...row)) / (s * s), (s * s) / (side * side * Math.min(...row)));
  }

  function squarify(items, x, y, w, h) {
    const total = items.reduce((s, i) => s + i.size, 0);
    const areas = items.map((i) => (i.size / total) * w * h);
    const rects = [];
    let i = 0;
    while (i < items.length) {
      const side = Math.min(w, h);
      const row = [areas[i]];
      let j = i + 1;
      let cur = worst(row, side);
      while (j < items.length) {
        const next = worst(row.concat(areas[j]), side);
        if (next > cur) break;
        row.push(areas[j]);
        cur = next;
        j++;
      }
      const sum = row.reduce((a, b) => a + b, 0);
      if (w >= h) {
        const cw = sum / h;
        let cy = y;
        row.forEach((a, k) => { const ch = a / cw; rects.push({ item: items[i + k], x, y: cy, w: cw, h: ch }); cy += ch; });
        x += cw; w -= cw;
      } else {
        const rh = sum / w;
        let cx = x;
        row.forEach((a, k) => { const cw = a / rh; rects.push({ item: items[i + k], x: cx, y, w: cw, h: rh }); cx += cw; });
        y += rh; h -= rh;
      }
      i = j;
    }
    return rects;
  }

  function treemap(root, onPick) {
    const MAX_TILES = 36;
    const kids = root.children.filter((c) => c.size > 0);
    if (!kids.length) return el('p', { class: 'empty', text: 'Keine Dateien mit Größe gefunden.' });
    let items = kids.slice(0, MAX_TILES);
    const rest = kids.slice(MAX_TILES);
    if (rest.length) {
      items.push({ name: 'weitere ' + nf.format(rest.length) + ' Ordner', path: null, size: rest.reduce((s, c) => s + c.size, 0), summary: false });
    }
    const total = items.reduce((s, c) => s + c.size, 0);
    const W = 1600; const H = 700;
    const box = el('div', { class: 'treemap', role: 'group', 'aria-label': 'Größe nach Ordner' });
    for (const r of squarify(items, 0, 0, W, H)) {
      const share = r.item.size / total;
      const small = r.w / W < 0.09 || r.h / H < 0.14;
      const tiny = r.w / W < 0.055 || r.h / H < 0.1;
      const tile = el('button', {
        type: 'button',
        class: 'tile' + (r.item.summary ? ' summary' : '') + (small ? ' small' : '') + (tiny ? ' tiny' : ''),
        style: 'left:' + (r.x / W * 100) + '%;top:' + (r.y / H * 100) + '%;width:' + (r.w / W * 100) + '%;height:' + (r.h / H * 100) + '%;--share:' + share.toFixed(4),
        title: r.item.name + ' – ' + bytes(r.item.size) + ' (' + pct(r.item.size / root.size) + ')' + (r.item.summary ? ' – nur Summe' : ''),
        onclick: () => r.item.path && onPick(r.item.path),
      }, el('span', { class: 'n', text: r.item.name }), el('span', { class: 's', text: bytes(r.item.size) }));
      box.append(tile);
    }
    return box;
  }

  /* ---------- Größenbaum ---------- */

  function treeView(root) {
    const PAGE = 200;
    const index = new Map();

    function row(node, parentSize, expandable, toggle) {
      const share = parentSize ? node.size / parentSize : 0;
      return el('div', { class: 'trow' },
        el('span', { class: 'name' },
          expandable ? toggle : el('span', { class: 'toggle-gap' }),
          el('span', { class: 'label', text: node.name, title: node.path }),
          node.summary ? el('span', { class: 'badge', text: 'nur Summe' }) : null,
          node.link ? el('span', { class: 'badge', text: 'Link' }) : null),
        el('span', { class: 'bar', 'aria-hidden': 'true' }, el('i', { style: 'width:' + Math.max(share * 100, share > 0 ? 1 : 0).toFixed(1) + '%' })),
        el('span', { class: 'num', text: bytes(node.size) }),
        el('span', { class: 'num', text: nf.format(node.files) }));
    }

    function item(node, parentSize) {
      const li = el('li');
      index.set(node.path, { li, open: null });
      const expandable = node.children.length > 0;
      let list = null;
      let isOpen = false;
      const toggle = el('button', { type: 'button', class: 'toggle', 'aria-expanded': 'false', 'aria-label': node.name + ' aufklappen', text: '+' });
      function setOpen(open) {
        isOpen = open;
        toggle.setAttribute('aria-expanded', String(open));
        toggle.textContent = open ? '−' : '+';
        if (open && !list) {
          list = el('ul');
          let shown = 0;
          const more = el('button', { type: 'button', class: 'more' });
          const add = () => {
            for (const child of node.children.slice(shown, shown + PAGE)) list.append(item(child, node.size));
            shown = Math.min(shown + PAGE, node.children.length);
            more.hidden = shown >= node.children.length;
            more.textContent = 'Weitere ' + nf.format(Math.min(PAGE, node.children.length - shown)) + ' anzeigen';
          };
          more.addEventListener('click', add);
          add();
          li.append(list, el('div', { class: 'tree-more' }, more));
        }
        if (list) { list.hidden = !open; }
      }
      toggle.addEventListener('click', () => setOpen(!isOpen));
      index.get(node.path).open = () => setOpen(true);
      li.append(row(node, parentSize, expandable, toggle));
      return li;
    }

    const ul = el('ul', { class: 'tree' });
    const top = item(root, root.size);
    ul.append(top);
    index.get(root.path).open();
    // Führt nur ein Ordner weiter, gleich mit aufklappen.
    for (let n = root; n.children.length === 1; n = n.children[0]) index.get(n.children[0].path).open();
    return {
      node: el('div', null,
        el('div', { class: 'tree-head' }, el('span', { text: 'Ordner' }), el('span', { text: 'Anteil' }), el('span', { text: 'Größe' }), el('span', { text: 'Dateien' })),
        ul),
      open(path) { const e = index.get(path); if (e) { e.open(); e.li.scrollIntoView({ block: 'center' }); } },
    };
  }

  /* ---------- Abschnitte ---------- */

  function renderOverview() {
    const o = data.overview;
    const figs = el('dl', { class: 'figures' },
      fig('Gesamtgröße', bytes(o.total_size)),
      fig('Dateien', nf.format(o.files)),
      fig('Ordner', nf.format(o.dirs)),
      fig('Nur Summe', bytes(o.summary_size), pct(o.summary_share) + ' der Gesamtgröße'),
      fig('Nur in der Cloud', nf.format(o.cloud_only_files), bytes(o.cloud_only_size)),
      fig('Verschwendet durch Duplikate', bytes(data.duplicates.total_wasted), null, data.duplicates.total_wasted > 0),
      fig('Fehler', nf.format(o.error_count), null, o.error_count > 0));
    const tree = state.tree;
    const base = drill(data.size_tree);
    const legend = 'Die Fläche zeigt die Größe der Ordner direkt unter ' + (base === data.size_tree ? 'der Wurzel' : base.path + ', weil darüber nur ein einziger Ordner liegt')
      + '. Schraffierte Flächen zählen nur als Summe, ihre Dateien sind nicht einzeln erfasst.';
    slot('overview').append(
      figs,
      el('h3', { text: 'Wo ist der Platz hin?' }),
      treemap(base, (path) => { tree.open(path); document.getElementById('tree').scrollIntoView(); }),
      el('p', { class: 'legend', text: legend }));
  }

  // Springt über Ordner, die nur genau einen Unterordner enthalten.
  function drill(node) {
    let n = node;
    while (n.children.length === 1 && !n.children[0].summary) n = n.children[0];
    return n;
  }

  function fig(label, value, note, bad) {
    return el('div', { class: bad ? 'bad' : '' }, el('dt', { text: label }), el('dd', null, value, note ? el('small', { text: ' ' + note }) : null));
  }

  function renderTree() {
    slot('tree').append(state.tree.node);
  }

  function renderTop() {
    const s = slot('top');
    s.append(el('h3', { text: 'Größte Dateien' }), dataTable(fileColumns, data.top_files, { empty: 'Keine Dateien gefunden.' }));
    s.append(el('h3', { text: 'Größte Ordner' }), dataTable([
      { label: 'Ordner', get: (d) => d.path, cls: 'path', render: (d) => [d.path, d.summary ? el('span', { class: 'badge', text: 'nur Summe' }) : null] },
      { label: 'Größe', num: true, get: (d) => d.size, render: (d) => bytes(d.size) },
      { label: 'Dateien', num: true, get: (d) => d.files, render: (d) => nf.format(d.files) },
    ], data.top_dirs, { empty: 'Keine Unterordner gefunden.' }));
  }

  function typeColumns(label) {
    return [
      { label, get: (t) => t.key },
      { label: 'Anzahl', num: true, get: (t) => t.count, render: (t) => nf.format(t.count) },
      { label: 'Größe', num: true, get: (t) => t.size, render: (t) => bytes(t.size) },
    ];
  }

  function renderTypes() {
    const s = slot('types');
    s.append(el('h3', { text: 'Nach Kategorie' }), dataTable(typeColumns('Kategorie'), data.file_types.by_category, { pageSize: 20 }));
    s.append(el('h3', { text: 'Nach Endung' }), dataTable(typeColumns('Endung'), data.file_types.by_extension, { pageSize: 50 }));
  }

  function days(n) {
    if (n % 365 === 0) return plural(n / 365, 'Jahr', 'Jahre');
    if (n % 30 === 0) return plural(n / 30, 'Monat', 'Monate');
    return plural(n, 'Tag', 'Tage');
  }

  function renderAge() {
    const s = slot('age');
    const a = data.age;
    const maxSize = Math.max(1, ...a.classes.map((c) => c.size));
    s.append(el('h3', { text: 'Nach Änderungsdatum' }), dataTable([
      { label: 'Alter', get: (c) => c.label },
      { label: 'Anteil', get: (c) => c.size, render: (c) => el('span', { class: 'bar', style: 'min-width:8rem' }, el('i', { style: 'width:' + (c.size / maxSize * 100).toFixed(1) + '%' })) },
      { label: 'Anzahl', num: true, get: (c) => c.count, render: (c) => nf.format(c.count) },
      { label: 'Größe', num: true, get: (c) => c.size, render: (c) => bytes(c.size) },
    ], a.classes, { pageSize: 10 }));
    s.append(
      el('h3', { text: 'Dateien, die älter sind als ' + days(a.old_after_days) }),
      el('p', { class: 'hint', text: plural(a.old_files.length, 'Datei', 'Dateien') + ' mit zusammen ' + bytes(a.old_total_size) + '. Die Schwelle ändert sich mit --old-after.' }),
      dataTable(fileColumns, a.old_files, { empty: 'Keine alten Dateien gefunden.' }));
  }

  function renderDuplicates() {
    const d = data.duplicates;
    const s = slot('duplicates');
    s.append(el('p', { class: 'hint', text: d.group_count === 0
      ? 'Keine Gruppen identischer Dateien gefunden.'
      : plural(d.group_count, 'Gruppe', 'Gruppen') + ' identischer Dateien, zusammen ' + bytes(d.total_wasted) + ' verschwendeter Platz. Hardlinks zählen als eine Datei.' }));
    if (d.group_count === 0) return;
    s.append(groupList(d.groups, (g) => ({
      title: bytes(g.wasted) + ' verschwendet',
      meta: plural(g.instances, 'Kopie', 'Kopien') + ' à ' + bytes(g.size),
      search: g.files.map((f) => f.path).join(' '),
      lines: g.files.map((f) => f.path + (f.nlinks > 1 ? '  (Hardlink)' : '') + '  · ' + date(f.mtime)),
    })));
  }

  function renderProbable() {
    const s = slot('probable');
    s.append(el('p', { class: 'hint', text: 'Gleicher Name und gleiche Größe, aber mindestens eine Datei liegt nur in der Cloud. Der Inhalt wurde nicht verglichen, damit nichts heruntergeladen wird.' }));
    s.append(groupList(data.probable_duplicates, (g) => ({
      title: g.name,
      meta: plural(g.files.length, 'Datei', 'Dateien') + ' à ' + bytes(g.size),
      badge: el('span', { class: 'badge signal', text: 'nicht verifiziert' }),
      search: g.files.map((f) => f.path).join(' '),
      lines: g.files.map((f) => f.path + (f.cloud_only ? '  (nur Cloud)' : '')),
    }), { empty: 'Keine wahrscheinlichen Duplikate gefunden.' }));
  }

  function renderSimilar() {
    const s = slot('similar');
    s.append(el('p', { class: 'hint', text: 'Dateien im selben Ordner, die sich nur durch Kopie-Zusätze, Versionsmarker oder Datumsangaben im Namen unterscheiden.' }));
    s.append(groupList(data.similar, (g) => ({
      title: g.files[0].name,
      meta: plural(g.files.length, 'Datei', 'Dateien') + ' in ' + g.dir,
      search: g.dir + ' ' + g.files.map((f) => f.name).join(' '),
      lines: g.files.map((f) => f.name + '  · ' + bytes(f.size) + '  · ' + date(f.mtime) + (f.exact_duplicate ? '  (exaktes Duplikat)' : '')),
    }), { empty: 'Keine ähnlichen Dateien gefunden.' }));
  }

  function renderStructure() {
    slot('structure').append(dataTable([
      { label: 'Ordner', get: (i) => i.path, cls: 'path' },
      { label: 'Befund', get: (i) => i.label },
      { label: 'Einträge', num: true, get: (i) => i.direct_entries, render: (i) => nf.format(i.direct_entries) },
      { label: 'Tiefe', num: true, get: (i) => i.depth },
    ], data.structure, {
      empty: 'Keine Auffälligkeiten in der Ordnerstruktur.',
      facet: { label: 'Befund', get: (i) => [i.label] },
    }));
  }

  function renderProblems() {
    slot('problems').append(dataTable([
      { label: 'Pfad', get: (p) => p.path, cls: 'path', render: (p) => [p.path, p.kind === 'dir' ? el('span', { class: 'badge', text: 'Ordner' }) : null] },
      { label: 'Probleme', get: (p) => p.labels.join(', ') },
      { label: 'Größe', num: true, get: (p) => p.size, render: (p) => bytes(p.size) },
    ], data.problems, {
      empty: 'Keine Problemdateien gefunden.',
      facet: { label: 'Problem', get: (p) => p.labels },
    }));
  }

  function renderErrors() {
    slot('errors').append(dataTable([
      { label: 'Pfad', get: (e) => e.path, cls: 'path' },
      { label: 'Art', get: (e) => e.kind },
      { label: 'Meldung', get: (e) => e.message },
    ], data.errors, { empty: 'Beim Scan sind keine Fehler aufgetreten.' }));
  }

  /* ---------- Verlauf ---------- */

  // Der SVG-Namensraum ist ein Bezeichner, keine Ressource: es wird nichts geladen.
  const SVG_NS = 'http://www.w3.org/2000/svg';
  function svgEl(tag, attrs) {
    const e = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
    return e;
  }

  function sparkline(values, opts) {
    const o = Object.assign({ w: 140, h: 32, min: null, max: null, label: 'Trend' }, opts);
    if (values.length < 2) return el('span', { class: 'muted', text: '–' });
    const lo = o.min != null ? o.min : Math.min(...values);
    const hi = o.max != null ? o.max : Math.max(...values);
    const span = hi - lo || 1;
    const pad = 3;
    const x = (i) => pad + (i / (values.length - 1)) * (o.w - 2 * pad);
    const y = (v) => o.h - pad - ((v - lo) / span) * (o.h - 2 * pad);
    const last = values.length - 1;
    const svg = svgEl('svg', { class: 'spark', viewBox: '0 0 ' + o.w + ' ' + o.h, width: o.w, height: o.h, role: 'img',
      'aria-label': o.label + ': ' + values.map((v) => nf.format(v)).join(', ') });
    svg.append(
      svgEl('polyline', { fill: 'none', stroke: 'currentColor', 'stroke-width': 2, 'stroke-linejoin': 'round',
        points: values.map((v, i) => x(i).toFixed(1) + ',' + y(v).toFixed(1)).join(' ') }),
      svgEl('circle', { fill: 'currentColor', r: 3, cx: x(last).toFixed(1), cy: y(values[last]).toFixed(1) }));
    return svg;
  }

  const shortDate = (iso) => new Date(iso).toLocaleDateString('de-DE', { day: '2-digit', month: '2-digit' });
  const signed = (n, fmt) => (n > 0 ? '+' : n < 0 ? '−' : '±') + (fmt || nf.format)(Math.abs(n));

  function renderHistory() {
    const h = data.history;
    const s = slot('history');
    if (!h) {
      s.append(el('p', { class: 'hint', text: 'Für diesen Bericht liegt kein Verlauf vor, zum Beispiel weil nur ein Unterordner einer gescannten Wurzel ausgewertet wird.' }));
      return;
    }
    const c = h.comparison;
    const change = !c ? 'erster Lauf' : signed(c.delta) + ' seit ' + shortDate(c.previous_at) + (c.kind === 'limited' ? ' (eingeschränkt vergleichbar)' : '');
    s.append(el('dl', { class: 'figures' }, fig('Health-Score', h.score + ' von 100', change, !!c && c.delta < 0)));
    for (const note of h.notes) s.append(el('p', { class: 'warn', text: note }));
    if (c && c.kind === 'limited') {
      s.append(el('p', { class: 'warn', text: 'Einstellungen oder Bewertungsformeln haben sich seit dem letzten Lauf geändert. Der Vergleich ist nur ein Anhaltspunkt.' }));
    }

    s.append(el('h3', { text: 'Die größten Abzüge' }));
    if (!h.deductions.length) {
      s.append(el('p', { class: 'hint', text: 'Keine nennenswerten Abzüge.' }));
    } else {
      s.append(el('ul', { class: 'deductions' }, h.deductions.map((d) =>
        el('li', null, el('strong', { text: '−' + new Intl.NumberFormat('de-DE', { maximumFractionDigits: 1 }).format(d.points) + ': ' }), d.text))));
    }
    s.append(el('p', { class: 'hint', text: 'Alte Daten außerhalb von _Archiv erscheinen nur als Kennzahl und senken den Score nicht.' }));

    s.append(el('h3', { text: 'Score im Zeitverlauf' }));
    s.append(h.series.length >= 2
      ? sparkline(h.series.map((p) => p.score), { w: 480, h: 90, min: 0, max: 100, label: 'Health-Score' })
      : el('p', { class: 'hint', text: 'Ein Trend entsteht ab dem zweiten Lauf.' }));

    s.append(el('h3', { text: 'Teilwerte' }), dataTable([
      { label: 'Teilwert', get: (p) => p.label },
      { label: 'Wert', get: (p) => (p.value == null ? -1 : p.value), render: (p) => p.value == null
        ? el('span', { class: 'muted', text: 'nicht bewertet' })
        : el('span', { class: 'bar', style: 'min-width:8rem' }, el('i', { style: 'width:' + p.value.toFixed(0) + '%' })) },
      { label: 'Punkte', num: true, get: (p) => (p.value == null ? -1 : p.value), render: (p) => (p.value == null ? '–' : nf.format(Math.round(p.value))) },
    ], h.parts, { pageSize: 10 }));

    const fmt = (m, v) => (m.unit === 'bytes' ? bytes(v) : nf.format(v));
    s.append(el('h3', { text: 'Kennzahlen' }), dataTable([
      { label: 'Kennzahl', get: (m) => m.label },
      { label: 'Jetzt', num: true, get: (m) => m.now, render: (m) => fmt(m, m.now) },
      { label: 'Letzter Lauf', num: true, get: (m) => (m.previous == null ? -1 : m.previous), render: (m) => (m.previous == null ? '–' : fmt(m, m.previous)) },
      { label: 'Veränderung', num: true, get: (m) => (m.previous == null ? 0 : m.now - m.previous),
        render: (m) => (m.previous == null ? '–' : signed(m.now - m.previous, (n) => fmt(m, n))) },
      { label: 'Trend', get: () => '', render: (m) => sparkline(h.series.map((p) => p.values[m.key] || 0), { label: m.label }) },
    ], h.metrics, { pageSize: 20 }));

    s.append(el('h3', { text: 'Ordner der ersten Ebene' }), dataTable([
      { label: 'Ordner', get: (f) => f.folder === '*' ? 'Sonstige' : f.folder, cls: 'path' },
      { label: 'Score', num: true, get: (f) => f.score },
      { label: 'Vorher', num: true, get: (f) => (f.previous_score == null ? -1 : f.previous_score), render: (f) => (f.previous_score == null ? '–' : f.previous_score) },
      { label: 'Größe', num: true, get: (f) => f.size, render: (f) => bytes(f.size) },
      { label: 'Dateien', num: true, get: (f) => f.files, render: (f) => nf.format(f.files) },
      { label: 'Größter Abzug', get: (f) => f.top_deduction || '' },
    ], h.folders, { pageSize: 25, empty: 'Keine Ordner auf der ersten Ebene.' }));
  }

  /* ---------- Kopf, Navigation, Farbschema ---------- */

  function renderHeader() {
    const m = data.meta;
    const when = [];
    if (m.scanned_at) when.push('Scan: ' + m.scanned_at);
    when.push('Bericht: ' + m.generated_at);
    document.getElementById('meta-when').textContent = when.join('  ·  ');
    if (m.scan_status !== 'complete') {
      const w = document.getElementById('meta-warn');
      w.hidden = false;
      w.textContent = 'Der Scan wurde nicht vollständig abgeschlossen. Die Zahlen in diesem Bericht sind unvollständig.';
    }
  }

  function renderNavCounts() {
    const counts = {
      duplicates: data.duplicates.group_count,
      probable: data.probable_duplicates.length,
      similar: data.similar.length,
      structure: data.structure.length,
      problems: data.problems.length,
      errors: data.errors.length,
    };
    document.querySelectorAll('.nav a').forEach((a) => {
      const id = a.getAttribute('href').slice(1);
      if (counts[id] != null) a.append(el('span', { class: 'count', text: nf.format(counts[id]) }));
    });
    const links = [...document.querySelectorAll('.nav a')];
    const sections = links.map((a) => document.getElementById(a.getAttribute('href').slice(1)));
    if ('IntersectionObserver' in window) {
      const obs = new IntersectionObserver((entries) => {
        for (const e of entries) {
          if (e.isIntersecting) links.forEach((a, i) => a.setAttribute('aria-current', String(sections[i] === e.target)));
        }
      }, { rootMargin: '-10% 0px -80% 0px' });
      sections.forEach((s) => s && obs.observe(s));
    }
  }

  function setupTheme() {
    const btn = document.getElementById('theme-toggle');
    const order = [null, 'light', 'dark'];
    const names = { null: 'Automatisch', light: 'Hell', dark: 'Dunkel' };
    let i = 0;
    try {
      const saved = localStorage.getItem('oc-theme');
      if (saved && order.includes(saved)) i = order.indexOf(saved);
    } catch (e) { /* ohne Speicher weiterarbeiten */ }
    const apply = () => {
      const t = order[i];
      if (t) document.documentElement.setAttribute('data-theme', t);
      else document.documentElement.removeAttribute('data-theme');
      btn.textContent = names[t];
    };
    btn.addEventListener('click', () => {
      i = (i + 1) % order.length;
      apply();
      try { if (order[i]) localStorage.setItem('oc-theme', order[i]); else localStorage.removeItem('oc-theme'); } catch (e) { /* egal */ }
    });
    apply();
  }

  const state = { tree: null };
  state.tree = treeView(data.size_tree);
  renderHeader();
  renderOverview();
  renderHistory();
  renderTree();
  renderTop();
  renderTypes();
  renderAge();
  renderDuplicates();
  renderProbable();
  renderSimilar();
  renderStructure();
  renderProblems();
  renderErrors();
  renderNavCounts();
  setupTheme();
})();
