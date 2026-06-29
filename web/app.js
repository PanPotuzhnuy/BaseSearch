// Base Search — local web client.
// Vanilla JS, no dependencies. Talks to the local server over /api/*, sending
// the per-session token (injected as window.__BASE_SEARCH_TOKEN) in the
// Authorization header. Renders the monochrome glass UI defined in app.css.
"use strict";
(() => {
  const TOKEN = window.__BASE_SEARCH_TOKEN || "";
  const SVGNS = "http://www.w3.org/2000/svg";

  // --------------------------------------------------------------- helpers
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

  function el(tag, attrs = {}, children = []) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (v == null || v === false) continue;
      if (k === "class") node.className = v;
      else if (k === "text") node.textContent = v;
      else if (k === "html") node.innerHTML = v;
      else if (k === "dataset") Object.assign(node.dataset, v);
      else if (k.startsWith("on") && typeof v === "function") node.addEventListener(k.slice(2), v);
      else node.setAttribute(k, v);
    }
    for (const c of [].concat(children)) {
      if (c == null || c === false) continue;
      node.appendChild(typeof c === "object" ? c : document.createTextNode(String(c)));
    }
    return node;
  }

  function icon(id, cls = "ic") {
    const svg = document.createElementNS(SVGNS, "svg");
    svg.setAttribute("class", cls);
    const use = document.createElementNS(SVGNS, "use");
    use.setAttribute("href", "#" + id);
    svg.appendChild(use);
    return svg;
  }

  const enUS = "en-US";
  const fmtInt = (n) => Math.round(Number(n) || 0).toLocaleString(enUS);
  function fmtCompact(n) {
    n = Number(n) || 0;
    const a = Math.abs(n);
    if (a >= 1e9) return (n / 1e9).toFixed(2) + "B";
    if (a >= 1e6) return (n / 1e6).toFixed(2) + "M";
    if (a >= 1e3) return (n / 1e3).toFixed(1) + "k";
    return Math.round(n).toLocaleString(enUS);
  }
  const fmtMoney = (n) => "$" + fmtCompact(n);
  function fmtPrice(n) {
    n = Number(n) || 0;
    return n >= 100 ? n.toFixed(0) : n.toFixed(2);
  }
  const t = (key, fallback) => state.strings[key] || fallback || key;

  function debounce(fn, ms) {
    let h;
    return (...a) => { clearTimeout(h); h = setTimeout(() => fn(...a), ms); };
  }

  async function api(path, params = {}) {
    const url = new URL(path, location.origin);
    for (const [k, v] of Object.entries(params)) {
      if (v != null && v !== "") url.searchParams.set(k, v);
    }
    const res = await fetch(url, { headers: { Authorization: "Bearer " + TOKEN } });
    if (!res.ok) {
      let msg = res.status + " " + res.statusText;
      try { const j = await res.json(); if (j && j.error) msg = j.error; } catch (e) { /* ignore */ }
      throw new Error(msg);
    }
    return res.json();
  }

  // --------------------------------------------------------------- state
  const FILTER_FIELDS = [
    ["year", "2024"], ["product_code", "SKU-42"], ["edrpou", "Company ID"],
    ["trademark", "Brand"], ["recipient", "Customer"], ["sender", "Supplier"],
    ["description", "Product name..."], ["origin_country", "CN"],
    ["dispatch_country", "PL"], ["trade_country", "IE"],
  ];

  // Column display order in the results grid (sensible first; the rest follow).
  const PREFERRED_COLS = [
    "declaration_date", "declaration_number", "recipient", "edrpou", "sender",
    "product_code", "description", "trademark", "currency_control_value",
    "net_kg", "gross_kg", "quantity", "unit", "origin_country",
    "dispatch_country", "trade_country", "rfv_usd_kg", "source_file",
  ];
  const NUM_COLS = new Set([
    "item_number", "quantity", "gross_kg", "net_kg", "declaration_weight",
    "currency_control_value", "field_43", "field_43_01", "rfv_usd_kg",
    "unit_weight", "weight_difference", "field_3001", "field_3002", "field_9610",
    "rmv_net_usd_kg", "rmv_usd_extra_unit", "rmv_gross_usd_kg", "min_base_usd_kg",
    "min_base_difference", "preferential", "full_rate",
  ]);
  const MONO_COLS = new Set(["declaration_number", "product_code", "edrpou"]);
  const COUNTRY_COLS = new Set(["origin_country", "dispatch_country", "trade_country"]);

  const state = {
    schema: [], colOrder: [], colIndex: {}, strings: {}, lang: "en",
    query: { text: "", filters: {} },
    page: 0, hasNext: false, shown: 0, total: null, searchGen: 0,
    tab: "results", group: "overview", anLimit: 10,
    an: null, anLoaded: new Set(), anGen: 0,
    pivot: { row: "recipient", col: "month", metric: "value" },
    busyCount: 0,
  };

  // --------------------------------------------------------------- busy + toast
  function busy(on) {
    state.busyCount = Math.max(0, state.busyCount + (on ? 1 : -1));
    $("#busy-bar").hidden = state.busyCount === 0;
  }
  let toastTimer;
  function toast(msg, isError) {
    const node = $("#toast");
    node.textContent = msg;
    node.classList.toggle("error", !!isError);
    node.hidden = false;
    requestAnimationFrame(() => node.classList.add("show"));
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      node.classList.remove("show");
      setTimeout(() => { node.hidden = true; }, 260);
    }, isError ? 5200 : 2600);
  }

  // --------------------------------------------------------------- i18n + theme
  function applyStrings() {
    $$("[data-i18n]").forEach((n) => {
      const v = state.strings[n.dataset.i18n];
      if (v) n.textContent = v;
    });
    $$("[data-i18n-ph]").forEach((n) => {
      const v = state.strings[n.dataset.i18nPh];
      if (v) n.setAttribute("placeholder", v);
    });
  }

  async function loadI18n(lang) {
    const data = await api("/api/i18n", { lang });
    state.lang = data.lang;
    state.strings = data.strings || {};
    document.documentElement.lang = data.lang;
    const sel = $("#lang-select");
    if (!sel.options.length) {
      (data.languages || []).forEach((l) => sel.appendChild(el("option", { value: l.code, text: l.label })));
    }
    sel.value = data.lang;
    applyStrings();
    buildFilters();
  }

  function applyTheme(theme) {
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem("bs-theme", theme); } catch (e) { /* ignore */ }
    const use = $("#theme-toggle use");
    if (use) use.setAttribute("href", theme === "dark" ? "#i-moon" : "#i-sun");
  }

  // --------------------------------------------------------------- filters
  function buildFilters() {
    const wrap = $("#filter-fields");
    wrap.textContent = "";
    for (const [name, ph] of FILTER_FIELDS) {
      const input = el("input", {
        id: "f-" + name, value: state.query.filters[name] || "",
        placeholder: ph, autocomplete: "off", spellcheck: "false",
        oninput: onFilterInput, onchange: () => runSearch(true),
      });
      input.dataset.field = name;
      wrap.appendChild(el("label", { class: "field" }, [
        el("span", { text: t(name, name) }), input,
      ]));
    }
    syncFilterChrome();
  }

  const onFilterInput = debounce((e) => {
    state.query.filters[e.target.dataset.field] = e.target.value.trim();
    syncFilterChrome();
    runSearch(true);
  }, 350);

  function syncFilterChrome() {
    let count = 0;
    for (const [name] of FILTER_FIELDS) {
      const input = $("#f-" + name);
      const filled = !!(state.query.filters[name] || "").trim();
      if (input) input.parentElement.classList.toggle("filled", filled);
      if (filled) count++;
    }
    const badge = $("#filters-count");
    badge.textContent = count;
    badge.hidden = count === 0;
  }

  function clearFilters() {
    state.query.filters = {};
    for (const [name] of FILTER_FIELDS) { const i = $("#f-" + name); if (i) i.value = ""; }
    syncFilterChrome();
    runSearch(true);
  }

  function applyFilterAction(action) {
    if (!action) return;
    state.query.filters[action.field] = action.value;
    const input = $("#f-" + action.field);
    if (input) input.value = action.value;
    syncFilterChrome();
    switchTab("results");
    runSearch(true);
  }

  // --------------------------------------------------------------- search
  function currentParams(extra = {}) {
    return Object.assign({ text: state.query.text }, state.query.filters, extra);
  }

  function setSchema(columns) {
    if (!Array.isArray(columns)) return;
    const oldSig = state.schema.map((c) => c.name).join("\u001f");
    const newSig = columns.map((c) => c.name).join("\u001f");
    state.schema = columns;
    if (oldSig !== newSig) {
      state.colOrder = [];
      state.colIndex = {};
    }
  }

  const runSearchDebounced = debounce(() => runSearch(true), 380);

  async function runSearch(resetPage) {
    if (resetPage) state.page = 0;
    state.an = null;
    state.anLoaded.clear();
    state.anGen++;
    const gen = ++state.searchGen;
    state.total = null;
    renderActiveFilters();
    if (state.tab === "analytics") loadAnalytics();
    busy(true);
    try {
      const data = await api("/api/search", currentParams({ page: state.page, limit: 100 }));
      if (gen !== state.searchGen) return;
      setSchema(data.columns);
      state.hasNext = data.has_next;
      state.shown = data.rows.length;
      renderResults(data);
      $("#tab-meta").textContent = `${data.elapsed_ms} ms`;
      updateResultsMeta();
      updatePager();
      fetchCount(gen);
    } catch (e) {
      toast(e.message, true);
    } finally {
      busy(false);
    }
  }

  // The exact total is fetched separately so paging stays instant; it fills in
  // a moment later and is ignored if the query has already changed.
  async function fetchCount(gen) {
    try {
      const data = await api("/api/count", currentParams());
      if (gen !== state.searchGen) return;
      state.total = data.total;
      updateResultsMeta();
      updatePager();
    } catch (e) { /* count is a non-critical enhancement */ }
  }

  function updateResultsMeta() {
    const meta = $("#results-meta");
    if (!state.shown) { meta.textContent = state.total === 0 ? t("nothing_found", "Nothing found.") : ""; return; }
    const start = state.page * 100 + 1;
    const end = state.page * 100 + state.shown;
    const range = `${fmtInt(start)}–${fmtInt(end)}`;
    meta.textContent = state.total != null
      ? `${range} ${t("of_label", "of")} ${fmtInt(state.total)} ${t("rows_label", "rows")}`
      : `${range}${state.hasNext ? "+" : ""} ${t("rows_label", "rows")}`;
  }

  function renderActiveFilters() {
    const bar = $("#active-filters");
    bar.textContent = "";
    const chips = [];
    if (state.query.text.trim()) chips.push(["__text", state.query.text.trim()]);
    for (const [name] of FILTER_FIELDS) {
      const v = (state.query.filters[name] || "").trim();
      if (v) chips.push([name, v]);
    }
    if (!chips.length) { bar.hidden = true; return; }
    bar.hidden = false;
    chips.forEach(([key, val]) => {
      const isText = key === "__text";
      const chip = el("span", { class: "fchip" }, [
        isText ? icon("i-search", "ic fchip-ic") : el("span", { class: "fchip-k", text: t(key, key) }),
        el("span", { class: "fchip-v", text: val, title: val }),
        el("button", { class: "fchip-x", type: "button", "aria-label": "Remove", onclick: () => removeFilter(key) }, [icon("i-x")]),
      ]);
      bar.appendChild(chip);
    });
    bar.appendChild(el("button", { class: "fchip-clear", type: "button", onclick: clearAllQuery }, [t("clear_filters", "Clear all")]));
  }

  function removeFilter(key) {
    if (key === "__text") {
      state.query.text = ""; $("#q").value = ""; $("#clear-q").hidden = true;
    } else {
      state.query.filters[key] = ""; const i = $("#f-" + key); if (i) i.value = "";
    }
    syncFilterChrome();
    runSearch(true);
  }

  function clearAllQuery() {
    state.query.text = ""; $("#q").value = ""; $("#clear-q").hidden = true;
    state.query.filters = {};
    for (const [name] of FILTER_FIELDS) { const i = $("#f-" + name); if (i) i.value = ""; }
    syncFilterChrome();
    runSearch(true);
  }

  function orderedColumns() {
    if (state.colOrder.length) return state.colOrder;
    const byName = {};
    state.schema.forEach((c, i) => { byName[c.name] = i; state.colIndex[c.name] = i; });
    const seen = new Set();
    const order = [];
    for (const name of PREFERRED_COLS) {
      if (name in byName) { order.push(byName[name]); seen.add(name); }
    }
    state.schema.forEach((c, i) => { if (!seen.has(c.name)) order.push(i); });
    state.colOrder = order;
    return order;
  }

  function isNumericColumn(col) {
    return NUM_COLS.has(col.name) || col.kind === "number";
  }

  function isMonoColumn(col) {
    return MONO_COLS.has(col.name) || col.kind === "code";
  }

  function renderResults(data) {
    const table = $("#results-table");
    const thead = table.tHead, tbody = table.tBodies[0];
    const ph = $("#results-placeholder");
    thead.textContent = ""; tbody.textContent = "";

    if (!data.rows.length) {
      ph.textContent = t("nothing_found", "Nothing found.");
      ph.classList.add("center");
      updatePager();
      return;
    }
    ph.classList.remove("center"); ph.textContent = "";

    const order = orderedColumns();
    const htr = el("tr");
    for (const ci of order) {
      const col = state.schema[ci];
      const th = el("th", { title: col.glossary || "", text: col.label || col.name });
      if (isNumericColumn(col)) th.classList.add("num");
      htr.appendChild(th);
    }
    thead.appendChild(htr);

    const frag = document.createDocumentFragment();
    data.rows.forEach((row, ri) => {
      const tr = el("tr", { class: "in", style: `animation-delay:${Math.min(ri, 24) * 9}ms` });
      if (row.duplicate_of) {
        tr.classList.add("dup");
        tr.title = `${t("dup_first_seen", "First seen in")} ${row.duplicate_of}`;
      }
      order.forEach((ci, k) => {
        const col = state.schema[ci];
        const val = row.cells[ci] || "";
        const td = el("td");
        if (isNumericColumn(col)) td.classList.add("num");
        else if (isMonoColumn(col)) td.classList.add("mono");
        else if (col.name === "description") td.classList.add("muted");

        if (col.name === "edrpou" && val) {
          td.appendChild(el("span", {
            class: "col-link", text: val,
            onclick: (e) => { e.stopPropagation(); openCompany(val); },
          }));
        } else if (COUNTRY_COLS.has(col.name) && val) {
          td.appendChild(el("span", { class: "cc", text: val }));
        } else {
          td.textContent = val;
          if (k === 0 && row.duplicate_of) td.appendChild(el("span", { class: "dup-tag", text: "dup" }));
        }
        tr.appendChild(td);
      });
      tr.addEventListener("click", () => openCard(row.id));
      frag.appendChild(tr);
    });
    tbody.appendChild(frag);
    updatePager();
  }

  function updatePager() {
    const pages = state.total != null ? Math.max(1, Math.ceil(state.total / 100)) : null;
    $("#page-label").textContent = pages ? `${state.page + 1} / ${pages}` : `${state.page + 1}`;
    $("#prev-page").disabled = state.page === 0;
    $("#next-page").disabled = !state.hasNext;
    $("#prev-page").style.opacity = state.page === 0 ? ".4" : "";
    $("#next-page").style.opacity = state.hasNext ? "" : ".4";
  }

  // --------------------------------------------------------------- analytics
  function switchTab(tab) {
    state.tab = tab;
    $$(".tab").forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
    $("#results-panel").classList.toggle("active", tab === "results");
    $("#analytics-panel").classList.toggle("active", tab === "analytics");
    if (tab === "analytics") loadAnalytics();
  }

  function switchGroup(group) {
    state.group = group;
    $$("#analytics-subtabs .chip").forEach((c) => c.classList.toggle("active", c.dataset.group === group));
    loadAnalytics();
  }

  const SCOPE_OF = { overview: "overview", companies: "companies", goods: "products", countries: "countries", prices: "prices" };

  async function loadAnalytics() {
    const scroll = $("#analytics-scroll");
    const queryEmpty = !state.query.text.trim() && !Object.values(state.query.filters).some((v) => (v || "").trim());
    if (queryEmpty) {
      scroll.textContent = "";
      scroll.appendChild(el("div", { class: "placeholder", text: t("analytics_hint", "Enter a query or filter to build analytics.") }));
      return;
    }
    if (state.group === "pivot") return loadPivot();

    const scope = SCOPE_OF[state.group];
    if (!state.anLoaded.has(state.group)) {
      const gen = state.anGen;
      busy(true);
      scroll.textContent = "";
      scroll.appendChild(el("div", { class: "placeholder", text: t("searching", "Searching…") }));
      try {
        const data = await api("/api/analytics", currentParams({ scope, limit: state.anLimit, hs_level: 10 }));
        if (gen !== state.anGen) return;
        if (data.needs_query) {
          scroll.textContent = "";
          scroll.appendChild(el("div", { class: "placeholder", text: data.message || "" }));
          return;
        }
        const a = data.analytics;
        if (!state.an) state.an = a;
        state.an.overview = a.overview;
        state.an.months = a.months;
        if (scope === "companies") state.an.company_sections = a.company_sections;
        if (scope === "products") state.an.product_sections = a.product_sections;
        if (scope === "countries") state.an.country_sections = a.country_sections;
        if (scope === "prices") state.an.price_sections = a.price_sections;
        state.anLoaded.add(state.group);
      } catch (e) {
        toast(e.message, true);
        scroll.textContent = "";
        scroll.appendChild(el("div", { class: "placeholder", text: e.message }));
        return;
      } finally {
        busy(false);
      }
    }
    renderAnalytics();
  }

  function summaryBar() {
    const o = state.an.overview;
    return el("div", { class: "an-summary" }, [
      kvInline(t("rows_label", "rows"), fmtInt(o.row_count)),
      kvInline(t("declarations_label", "declarations"), fmtInt(o.declaration_count)),
      kvInline(t("total_value", "value"), fmtMoney(o.total_value_usd)),
      kvInline(t("net_weight", "net kg"), fmtCompact(o.total_net_kg)),
      kvInline(t("avg_value_kg", "avg $/kg"), fmtPrice(o.avg_value_per_net_kg)),
    ]);
  }
  function kvInline(k, v) {
    return el("span", {}, [k + " ", el("b", { text: v })]);
  }

  function renderAnalytics() {
    const scroll = $("#analytics-scroll");
    scroll.textContent = "";
    scroll.appendChild(summaryBar());
    const a = state.an;
    if (state.group === "overview") {
      scroll.appendChild(renderOverview(a));
    } else if (state.group === "companies") {
      scroll.appendChild(sectionGrid(a.company_sections));
    } else if (state.group === "goods") {
      scroll.appendChild(sectionGrid(a.product_sections));
    } else if (state.group === "countries") {
      scroll.appendChild(sectionGrid(a.country_sections));
    } else if (state.group === "prices") {
      scroll.appendChild(priceCard(a.price_sections));
    }
  }

  function renderOverview(a) {
    const o = a.overview;
    const tiles = el("div", { class: "tiles" }, [
      tile(t("rows_label", "Rows"), fmtInt(o.row_count), `${fmtInt(o.declaration_count)} ${t("declarations_label", "declarations")}`),
      tile(t("total_value", "Value"), fmtMoney(o.total_value_usd), `${fmtPrice(o.avg_value_per_net_kg)} ${t("avg_value_kg", "$/kg")}`),
      tile(t("net_weight", "Net weight"), fmtCompact(o.total_net_kg) + " kg", `${fmtCompact(o.total_gross_kg)} kg ${t("gross_weight", "gross")}`),
      tile(t("recipients_label", "Recipients"), fmtInt(o.distinct_recipients), `${fmtInt(o.distinct_edrpou)} EDRPOU`),
      tile(t("unique_senders", "Senders"), fmtInt(o.distinct_senders), `${fmtInt(o.distinct_trademarks)} ${t("trademark", "brands")}`),
      tile(t("product_code", "Codes"), fmtInt(o.distinct_product_codes), `${fmtInt(o.distinct_origin_countries)} ${t("origin_country", "origins")}`),
    ]);
    const card = el("div", { class: "an-card" }, [
      cardHead("i-info", t("tab_overview", "Overview")), tiles,
    ]);
    const chart = monthsChart(a.months);
    return el("div", {}, [card, chart]);
  }

  function tile(k, v, s) {
    return el("div", { class: "tile" }, [
      el("div", { class: "k", text: k }),
      el("div", { class: "v", text: v }),
      s ? el("div", { class: "s", text: s }) : null,
    ]);
  }

  function cardHead(iconId, title, allBtn) {
    return el("div", { class: "an-card-head" }, [
      icon(iconId), el("div", { class: "an-card-title", text: title }), allBtn || null,
    ]);
  }

  function monthsChart(months) {
    if (!months || !months.length) return el("div");
    const max = Math.max(...months.map((m) => m.total_value_usd), 1);
    const cols = months.map((m) => {
      const h = Math.max(3, Math.round((m.total_value_usd / max) * 112));
      const stem = el("div", { class: "stem", style: `height:${h}px` });
      const col = el("div", {
        class: "col",
        title: `${m.month} · ${fmtMoney(m.total_value_usd)} · ${fmtInt(m.declarations)} ${t("declarations_label", "declarations")} · ${fmtCompact(m.total_net_kg)} kg`,
      }, [stem, el("div", { class: "cap", text: m.month.slice(2) })]);
      return col;
    });
    return el("div", { class: "an-card", style: "margin-top:12px" }, [
      cardHead("i-info", t("months_section", "Monthly dynamics")),
      el("div", { class: "chart" }, cols),
    ]);
  }

  function sectionGrid(sections) {
    const grid = el("div", { class: "an-grid" });
    (sections || []).forEach((s, i) => grid.appendChild(sectionCard(s, i)));
    if (!grid.children.length) grid.appendChild(el("div", { class: "placeholder", text: t("nothing_found", "Nothing found.") }));
    return grid;
  }

  const SECTION_ICON = {
    edrpou: "i-company", recipients: "i-company", senders: "i-company",
    product_codes: "i-filter", trademarks: "i-filter", product_groups: "i-filter",
    origin_countries: "i-globe", dispatch_countries: "i-globe", trade_countries: "i-globe",
  };

  function sectionCard(section, idx) {
    const max = Math.max(...section.rows.map((r) => r.total_value_usd), 1);
    const bars = el("div", { class: "bars" });
    section.rows.slice(0, 8).forEach((r) => {
      const fill = el("div", { class: "bar-fill", style: `width:${Math.max(2, (r.total_value_usd / max) * 100)}%` });
      const row = el("div", {
        class: "bar-row",
        title: `${fmtInt(r.rows)} ${t("rows_label", "rows")} · ${fmtInt(r.declarations)} ${t("declarations_label", "decl")} · ${fmtCompact(r.total_net_kg)} kg · ${fmtPrice(r.avg_value_per_net_kg)} $/kg`,
        onclick: () => applyFilterAction(r.filter_action),
      }, [
        fill,
        el("div", { class: "bar-label", text: r.label || "—" }),
        el("div", { class: "bar-val", text: fmtMoney(r.total_value_usd) }),
      ]);
      bars.appendChild(row);
    });
    const allBtn = el("button", {
      class: "btn ghost sm", onclick: () => openSection(section.kind, section.title),
    }, [t("all_label", "All")]);
    const card = el("div", { class: "an-card", style: `animation-delay:${idx * 40}ms` }, [
      cardHead(SECTION_ICON[section.kind] || "i-filter", section.title, allBtn), bars,
    ]);
    return card;
  }

  function priceCard(metrics) {
    const grid = el("div", { class: "an-grid" });
    const card = el("div", { class: "an-card", style: "grid-column:1/-1" }, [cardHead("i-info", t("prices_section", "Prices"))]);
    const table = el("table", { class: "ptable" });
    table.appendChild(el("thead", {}, el("tr", {}, [
      th(t("col_label", "Metric"), true), th(t("median", "Median")), th(t("weighted_avg", "Wtd avg")),
      th("P25"), th("P75"), th("Avg"), th("n"),
    ])));
    const tb = el("tbody");
    (metrics || []).forEach((m) => {
      tb.appendChild(el("tr", {}, [
        td(m.title, "left"), td(fmtPrice(m.median), "mono"), td(fmtPrice(m.weighted_average), "mono"),
        td(fmtPrice(m.p25), "mono"), td(fmtPrice(m.p75), "mono"), td(fmtPrice(m.average), "mono"), td(fmtInt(m.count), "mono"),
      ]));
    });
    table.appendChild(tb);
    card.appendChild(table);
    grid.appendChild(card);
    return grid;
  }
  function th(text, left) { const n = el("th", { text }); if (left) n.style.textAlign = "left"; return n; }
  function td(text, cls) {
    const n = el("td", { text });
    if (cls === "left") n.style.textAlign = "left";
    else if (cls === "mono") n.classList.add("mono");
    return n;
  }

  // --------------------------------------------------------------- pivot
  const PIVOT_DIMS = [
    ["recipient", "Recipient"], ["sender", "Sender"], ["edrpou", "EDRPOU"],
    ["product_code", "Product code"], ["trademark", "Trademark"],
    ["origin_country", "Origin"], ["dispatch_country", "Dispatch"], ["trade_country", "Trade"],
    ["month", "Month"], ["year", "Year"],
  ];
  const PIVOT_METRICS = [["value", "Value $"], ["rows", "Rows"], ["net_kg", "Net kg"]];

  async function loadPivot() {
    const scroll = $("#analytics-scroll");
    scroll.textContent = "";
    scroll.appendChild(summaryBar ? (state.an ? summaryBar() : el("div")) : el("div"));
    const controls = el("div", { class: "pivot-controls" }, [
      pivotSelect("row", PIVOT_DIMS, state.pivot.row),
      pivotSelect("col", PIVOT_DIMS, state.pivot.col),
      pivotSelect("metric", PIVOT_METRICS, state.pivot.metric),
    ]);
    scroll.appendChild(controls);
    const holder = el("div", { id: "pivot-holder" });
    scroll.appendChild(holder);
    busy(true);
    try {
      const data = await api("/api/pivot", currentParams(state.pivot));
      if (data.needs_query) {
        holder.appendChild(el("div", { class: "placeholder", text: data.message || "" }));
        return;
      }
      holder.appendChild(pivotTable(data.pivot));
    } catch (e) {
      toast(e.message, true);
    } finally {
      busy(false);
    }
  }

  function pivotSelect(key, options, value) {
    const sel = el("select", {
      class: "mini-select",
      onchange: (e) => { state.pivot[key] = e.target.value; loadPivot(); },
    });
    options.forEach(([v, label]) => {
      const opt = el("option", { value: v, text: label });
      if (v === value) opt.selected = true;
      sel.appendChild(opt);
    });
    return el("label", {}, [el("span", { text: key }), sel]);
  }

  function pivotTable(p) {
    const wrap = el("div", { class: "pivot-wrap" });
    const table = el("table", { class: "pivot" });
    const head = el("tr", {}, [el("th", { text: "" })]);
    p.col_labels.forEach((c) => head.appendChild(el("th", { text: c })));
    head.appendChild(el("th", { class: "tot", text: "Σ" }));
    table.appendChild(el("thead", {}, head));
    const tb = el("tbody");
    const max = Math.max(...p.cells.flat(), 1);
    p.row_labels.forEach((rl, ri) => {
      const tr = el("tr", {}, [el("th", { text: rl })]);
      p.cells[ri].forEach((v) => {
        const cell = el("td", { text: v ? fmtCompact(v) : "" });
        if (v) cell.style.background = `color-mix(in srgb, var(--acc) ${Math.round((v / max) * 55)}%, transparent)`;
        tr.appendChild(cell);
      });
      tr.appendChild(el("td", { class: "tot", text: fmtCompact(p.row_totals[ri]) }));
      tb.appendChild(tr);
    });
    const foot = el("tr", {}, [el("th", { class: "tot", text: "Σ" })]);
    p.col_totals.forEach((v) => foot.appendChild(el("td", { class: "tot", text: fmtCompact(v) })));
    foot.appendChild(el("td", { class: "tot", text: fmtCompact(p.grand_total) }));
    tb.appendChild(foot);
    table.appendChild(tb);
    wrap.appendChild(table);
    return wrap;
  }

  // --------------------------------------------------------------- drawer
  function openDrawer() {
    $("#scrim").hidden = false;
    const d = $("#drawer");
    d.classList.add("open");
    d.setAttribute("aria-hidden", "false");
  }
  function closeDrawer() {
    $("#scrim").hidden = true;
    const d = $("#drawer");
    d.classList.remove("open");
    d.setAttribute("aria-hidden", "true");
  }

  async function openCard(id) {
    openDrawer();
    $("#drawer-title").textContent = t("details", "Details");
    $("#drawer-sub").textContent = "#" + id;
    const body = $("#drawer-body");
    body.textContent = t("searching", "Searching…");
    try {
      const data = await api("/api/card", { id });
      body.textContent = "";
      $("#drawer-sub").textContent = data.source_file || ("#" + id);
      const kv = el("div", { class: "kv" });
      data.fields.forEach((f) => {
        if (!f.value) return;
        kv.appendChild(el("div", { class: "kv-row" + (f.extra ? " extra" : "") }, [
          el("div", { class: "k", text: f.label }),
          el("div", { class: "v", text: f.value }),
        ]));
      });
      body.appendChild(kv);
    } catch (e) {
      body.textContent = e.message;
    }
  }

  async function openCompany(edrpou) {
    openDrawer();
    $("#drawer-title").textContent = t("company_profile", "Company profile");
    $("#drawer-sub").textContent = edrpou;
    const body = $("#drawer-body");
    body.textContent = t("searching", "Searching…");
    try {
      const data = await api("/api/company", { edrpou, limit: 10 });
      const p = data.profile, o = p.overview;
      body.textContent = "";
      if (p.names && p.names.length) {
        body.appendChild(el("div", { class: "kv-section", text: "Names" }));
        body.appendChild(el("div", { class: "v", text: p.names.join(" · ") }));
      }
      body.appendChild(el("div", { class: "tiles", style: "margin-top:12px" }, [
        tile(t("rows_label", "Rows"), fmtInt(o.row_count), `${fmtInt(o.declaration_count)} decl`),
        tile(t("total_value", "Value"), fmtMoney(o.total_value_usd), `${fmtPrice(o.avg_value_per_net_kg)} $/kg`),
        tile(t("net_weight", "Net kg"), fmtCompact(o.total_net_kg), `${fmtInt(o.distinct_product_codes)} codes`),
      ]));
      body.appendChild(monthsChart(p.months));
      (p.product_sections || []).forEach((s) => body.appendChild(sectionCard(s, 0)));
      (p.country_sections || []).slice(0, 1).forEach((s) => body.appendChild(sectionCard(s, 0)));
    } catch (e) {
      body.textContent = e.message;
    }
  }

  // --------------------------------------------------------------- section modal
  let modalRows = [], modalSort = { key: "total_value_usd", dir: -1 };

  function openModalUI() { $("#modal-scrim").hidden = false; }
  function closeModal() { $("#modal-scrim").hidden = true; }

  async function openSection(kind, title) {
    openModalUI();
    $("#modal-title").textContent = title;
    $("#modal-search").value = "";
    $("#modal-meta").textContent = t("searching", "Searching…");
    $("#modal-body").textContent = "";
    try {
      const data = await api("/api/section", currentParams({ kind, hs_level: 10, limit: 20000 }));
      if (data.needs_query) { $("#modal-meta").textContent = data.message || ""; return; }
      modalRows = data.rows;
      $("#modal-meta").textContent =
        `${fmtInt(data.count)} ${t("showing", "shown")}${data.limited ? " · 20000 max" : ""}`;
      renderModalTable("");
    } catch (e) {
      $("#modal-meta").textContent = e.message;
    }
  }

  function renderModalTable(filter) {
    const body = $("#modal-body");
    body.textContent = "";
    const f = filter.trim().toLowerCase();
    let rows = f ? modalRows.filter((r) => (r.label || "").toLowerCase().includes(f)) : modalRows.slice();
    rows.sort((a, b) => (a[modalSort.key] < b[modalSort.key] ? 1 : -1) * modalSort.dir);
    rows = rows.slice(0, 500);

    const table = el("table", { class: "grid" });
    const cols = [
      ["label", t("col_label", "Name"), false], ["rows", t("rows_label", "Rows"), true],
      ["declarations", t("declarations_label", "Decl"), true], ["companies", t("col_companies", "Cos"), true],
      ["total_value_usd", t("total_value", "Value"), true], ["total_net_kg", t("net_weight", "Net kg"), true],
      ["share_percent", t("col_share", "Share"), true], ["avg_value_per_net_kg", t("avg_value_kg", "$/kg"), true],
    ];
    const htr = el("tr");
    cols.forEach(([key, label, num]) => {
      const th = el("th", { text: label });
      if (num) th.classList.add("num");
      th.style.cursor = "pointer";
      th.addEventListener("click", () => {
        modalSort.dir = modalSort.key === key ? -modalSort.dir : -1;
        modalSort.key = key;
        renderModalTable($("#modal-search").value);
      });
      htr.appendChild(th);
    });
    table.appendChild(el("thead", {}, htr));
    const tb = el("tbody");
    rows.forEach((r) => {
      const tr = el("tr", { onclick: () => { closeModal(); applyFilterAction(r.filter_action); } }, [
        cell(r.label || "—"), cellN(fmtInt(r.rows)), cellN(fmtInt(r.declarations)), cellN(fmtInt(r.companies)),
        cellN(fmtMoney(r.total_value_usd)), cellN(fmtCompact(r.total_net_kg)),
        cellN(r.share_percent.toFixed(1) + "%"), cellN(fmtPrice(r.avg_value_per_net_kg)),
      ]);
      tb.appendChild(tr);
    });
    table.appendChild(tb);
    body.appendChild(table);
  }
  function cell(text) { return el("td", { text }); }
  function cellN(text) { const n = el("td", { text }); n.classList.add("num"); return n; }

  function copyModal() {
    const f = $("#modal-search").value.trim().toLowerCase();
    let rows = f ? modalRows.filter((r) => (r.label || "").toLowerCase().includes(f)) : modalRows;
    const header = ["Name", "Rows", "Declarations", "Companies", "Value", "Net kg", "Share %", "$/kg"].join("\t");
    const lines = rows.slice(0, 5000).map((r) =>
      [r.label, r.rows, r.declarations, r.companies, Math.round(r.total_value_usd), Math.round(r.total_net_kg), r.share_percent.toFixed(1), r.avg_value_per_net_kg.toFixed(2)].join("\t"));
    navigator.clipboard.writeText([header, ...lines].join("\n"))
      .then(() => toast(t("copy_visible", "Copied")))
      .catch(() => toast("Clipboard blocked", true));
  }

  // --------------------------------------------------------------- export
  async function exportCsv() {
    busy(true);
    try {
      const url = new URL("/api/export-page.csv", location.origin);
      for (const [k, v] of Object.entries(currentParams({ page: state.page, limit: 100 }))) {
        if (v != null && v !== "") url.searchParams.set(k, v);
      }
      const res = await fetch(url, { headers: { Authorization: "Bearer " + TOKEN } });
      if (!res.ok) throw new Error(res.statusText);
      const blob = await res.blob();
      const a = el("a", { href: URL.createObjectURL(blob), download: "base-search-page.csv" });
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    } catch (e) {
      toast(e.message, true);
    } finally {
      busy(false);
    }
  }

  // --------------------------------------------------------------- stats
  async function loadStats() {
    try {
      const data = await api("/api/stats");
      $("#live-dot").classList.add("on");
      const status = data.total_rows
        ? `${fmtInt(data.total_rows)} ${t("rows_label", "rows")}`
        : t("db_rows", "Local database");
      $("#db-status").textContent = status;
    } catch (e) {
      $("#db-status").textContent = e.message;
    }
  }

  // --------------------------------------------------------------- events
  function wire() {
    $("#search-form").addEventListener("submit", (e) => { e.preventDefault(); runSearch(true); });
    const q = $("#q");
    q.addEventListener("input", () => {
      state.query.text = q.value;
      $("#clear-q").hidden = !q.value;
      runSearchDebounced();
    });
    $("#clear-q").addEventListener("click", () => {
      q.value = ""; state.query.text = ""; $("#clear-q").hidden = true; runSearch(true); q.focus();
    });
    $("#clear-filters").addEventListener("click", clearFilters);
    $("#prev-page").addEventListener("click", () => { if (state.page > 0) { state.page--; runSearch(false); } });
    $("#next-page").addEventListener("click", () => { if (state.hasNext) { state.page++; runSearch(false); } });
    $$(".tab").forEach((b) => b.addEventListener("click", () => switchTab(b.dataset.tab)));
    $$("#analytics-subtabs .chip").forEach((c) => c.addEventListener("click", () => switchGroup(c.dataset.group)));
    $("#analytics-limit").addEventListener("change", (e) => {
      state.anLimit = parseInt(e.target.value, 10) || 10;
      state.anLoaded.clear();
      loadAnalytics();
    });
    $("#export-btn").addEventListener("click", exportCsv);
    $("#refresh-btn").addEventListener("click", () => { loadStats(); runSearch(false); });
    $("#theme-toggle").addEventListener("click", () => {
      applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
    });
    $("#lang-select").addEventListener("change", (e) => loadI18n(e.target.value));
    $("#drawer-close").addEventListener("click", closeDrawer);
    $("#scrim").addEventListener("click", closeDrawer);
    $("#modal-close").addEventListener("click", closeModal);
    $("#modal-copy").addEventListener("click", copyModal);
    $("#modal-scrim").addEventListener("click", (e) => { if (e.target.id === "modal-scrim") closeModal(); });
    $("#modal-search").addEventListener("input", (e) => renderModalTable(e.target.value));
    // mobile filters
    $("#filters-toggle").addEventListener("click", () => $("#filters").classList.toggle("open"));
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") { closeModal(); closeDrawer(); }
      if (e.key === "/" && document.activeElement !== q && document.activeElement.tagName !== "INPUT") {
        e.preventDefault(); q.focus();
      }
    });
  }

  // --------------------------------------------------------------- init
  async function init() {
    let theme = "dark";
    try { theme = localStorage.getItem("bs-theme") || "dark"; } catch (e) { /* ignore */ }
    applyTheme(theme);
    wire();
    try {
      const schema = await api("/api/schema");
      setSchema(schema.columns || []);
    } catch (e) { toast(e.message, true); }
    await loadI18n("en").catch((e) => toast(e.message, true));
    loadStats();
    // Show the latest rows immediately instead of an empty screen.
    runSearch(true);
    $("#q").focus();
  }

  init();
})();
