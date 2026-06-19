const tokenFromUrl = new URLSearchParams(location.search).get("token");
if (tokenFromUrl) {
  sessionStorage.setItem("baseSearchToken", tokenFromUrl);
  history.replaceState(null, "", location.pathname);
}

const state = {
  token: sessionStorage.getItem("baseSearchToken") || "",
  page: 0,
  limit: 100,
  hasNext: false,
  columns: [],
  columnIndex: new Map(),
  rows: [],
  activeTab: "results",
  analyticsGroup: "overview",
  lang: localStorage.getItem("baseSearchLang") || "ua",
  i18n: {},
  languages: [],
  langLabel: "Language",
  section: null,
};

const visibleColumns = [
  "declaration_date",
  "declaration_number",
  "recipient",
  "edrpou",
  "sender",
  "product_code",
  "description",
  "origin_country",
  "dispatch_country",
  "trade_country",
  "currency_control_value",
  "net_kg",
  "rfv_usd_kg",
  "trademark",
  "source_file",
];

const numericColumns = new Set([
  "quantity",
  "gross_kg",
  "net_kg",
  "declaration_weight",
  "currency_control_value",
  "rfv_usd_kg",
  "unit_weight",
  "weight_difference",
  "rmv_net_usd_kg",
  "rmv_usd_extra_unit",
  "rmv_gross_usd_kg",
  "min_base_usd_kg",
]);

const fieldIds = [
  "text",
  "year",
  "product_code",
  "edrpou",
  "trademark",
  "recipient",
  "sender",
  "description",
  "origin_country",
  "dispatch_country",
  "trade_country",
];

const PRICE_KEY = {
  value_per_net_kg: "pm_value_per_net_kg",
  rfv_usd_kg: "pm_rfv",
  rmv_net_usd_kg: "pm_rmv_net",
  rmv_usd_extra_unit: "pm_rmv_extra_unit",
  rmv_gross_usd_kg: "pm_rmv_gross",
  min_base_usd_kg: "pm_min_base",
};

const $ = (id) => document.getElementById(id);
const fmtInt = new Intl.NumberFormat("uk-UA", { maximumFractionDigits: 0 });
const fmtNum = new Intl.NumberFormat("uk-UA", { maximumFractionDigits: 2 });
const fmtKg = new Intl.NumberFormat("uk-UA", { maximumFractionDigits: 3 });

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/** Replaces sequential "{}" placeholders, like the Rust fmt helper. */
function fmt(pattern, ...args) {
  let i = 0;
  return String(pattern ?? "").replace(/\{\}/g, () => (args[i++] ?? ""));
}

function t(key, fallback) {
  return (state.i18n && state.i18n[key]) || fallback || key;
}

function toast(message) {
  const node = $("toast");
  node.textContent = message;
  node.classList.add("show");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => node.classList.remove("show"), 3400);
}

function cleanParams(params) {
  const out = {};
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && String(value).trim() !== "") {
      out[key] = String(value).trim();
    }
  }
  return out;
}

async function api(path, params = {}) {
  if (!state.token) {
    throw new Error("Немає локального токена сесії. Запустіть браузерний режим заново.");
  }
  const qs = new URLSearchParams(cleanParams({ ...params, token: state.token }));
  const response = await fetch(`${path}?${qs.toString()}`, { cache: "no-store" });
  const data = await response.json();
  if (!response.ok || data.ok === false) {
    throw new Error(data.error || `HTTP ${response.status}`);
  }
  return data;
}

function collectQuery() {
  const query = {};
  for (const id of fieldIds) {
    query[id] = $(id).value.trim();
  }
  return query;
}

function queryIsEmpty(query = collectQuery()) {
  return Object.values(query).every((value) => !value);
}

function queryWithPaging(extra = {}) {
  return { ...collectQuery(), page: state.page, limit: state.limit, ...extra };
}

function cell(row, columnName) {
  const index = state.columnIndex.get(columnName);
  return index === undefined ? "" : row.cells[index] || "";
}

function columnLabel(name) {
  return state.columns.find((col) => col.name === name)?.label || name;
}

function sectionTitle(kind, fallback) {
  return t(`sec_${kind}`, fallback);
}

function priceTitle(kind, fallback) {
  return t(PRICE_KEY[kind] || "", fallback);
}

/* ---------- i18n ---------- */
async function loadI18n() {
  const data = await api("/api/i18n", { lang: state.lang });
  state.lang = data.lang || state.lang;
  localStorage.setItem("baseSearchLang", state.lang);
  state.i18n = data.strings || {};
  state.languages = data.languages || [];
  state.langLabel = data.language_label || "Language";
  applyI18n();
  buildLangSelect();
}

function applyI18n() {
  document.documentElement.lang = state.lang === "ua" ? "uk" : state.lang;
  for (const el of document.querySelectorAll("[data-i18n]")) {
    const value = state.i18n[el.dataset.i18n];
    if (value) el.textContent = value;
  }
  for (const el of document.querySelectorAll("[data-i18n-ph]")) {
    const value = state.i18n[el.dataset.i18nPh];
    if (value) el.placeholder = value;
  }
}

function buildLangSelect() {
  const select = $("lang-select");
  select.title = state.langLabel;
  select.innerHTML = state.languages
    .map(
      (l) => `<option value="${esc(l.code)}"${l.code === state.lang ? " selected" : ""}>${esc(l.label)}</option>`,
    )
    .join("");
}

async function onLangChange() {
  state.lang = $("lang-select").value;
  localStorage.setItem("baseSearchLang", state.lang);
  await loadI18n();
  await loadStats();
  renderTable();
  if (!queryIsEmpty()) {
    $("results-meta").textContent = "";
  }
  if (state.activeTab === "analytics") loadAnalytics();
}

/* ---------- schema / stats ---------- */
async function loadSchema() {
  const data = await api("/api/schema");
  state.columns = data.columns;
  state.columnIndex = new Map(data.columns.map((column, index) => [column.name, index]));
  renderTable();
}

async function loadStats() {
  const data = await api("/api/stats");
  let label = fmt(t("db_rows", "Database: {} rows"), fmtInt.format(data.total_rows));
  if (data.unindexed_rows > 0) label += ` · +${fmtInt.format(data.unindexed_rows)}`;
  $("db-status").textContent = label;
}

/* ---------- search / results ---------- */
async function search(resetPage = true) {
  if (resetPage) state.page = 0;
  $("results-meta").textContent = t("searching", "…");
  try {
    const data = await api("/api/search", queryWithPaging());
    state.rows = data.rows || [];
    state.hasNext = Boolean(data.has_next);
    $("results-meta").textContent =
      `${fmtInt.format(state.rows.length)} · ${fmt(t("search_ms", "in {} ms"), fmtInt.format(data.elapsed_ms))}`;
    $("page-label").textContent = String(state.page + 1);
    $("prev-page").disabled = state.page === 0;
    $("next-page").disabled = !state.hasNext;
    renderTable();
    if (state.activeTab === "analytics") loadAnalytics();
  } catch (err) {
    $("results-meta").textContent = err.message;
    toast(err.message);
  }
}

function renderTable() {
  const table = $("results-table");
  const head = table.querySelector("thead");
  const body = table.querySelector("tbody");
  const columns = visibleColumns.filter((name) => state.columnIndex.has(name));
  head.innerHTML = `<tr>${columns
    .map((name) => {
      const cls = numericColumns.has(name) ? ' class="num"' : "";
      return `<th${cls} title="${esc(columnLabel(name))}">${esc(columnLabel(name))}</th>`;
    })
    .join("")}</tr>`;

  if (!state.rows.length) {
    body.innerHTML = `<tr><td class="empty" colspan="${columns.length || 1}">${esc(t("nothing_found", "—"))}</td></tr>`;
    return;
  }

  body.innerHTML = "";
  for (const row of state.rows) {
    const tr = document.createElement("tr");
    if (row.duplicate_of) {
      tr.classList.add("duplicate");
      tr.title = fmt(t("dup_first_seen", "First seen in: {}"), row.duplicate_of);
    }
    tr.addEventListener("click", () => openCard(row.id));
    for (const name of columns) {
      const td = document.createElement("td");
      td.textContent = cell(row, name);
      if (!tr.classList.contains("duplicate")) td.title = td.textContent;
      if (numericColumns.has(name)) td.classList.add("num");
      if (name === "product_code" || name === "edrpou") td.classList.add("code");
      tr.appendChild(td);
    }
    body.appendChild(tr);
  }
}

/* ---------- drawer (record card / company) ---------- */
function openDrawer() {
  $("drawer").classList.add("open");
  $("drawer").setAttribute("aria-hidden", "false");
  $("backdrop").classList.add("show");
}

function closeDrawer() {
  $("drawer").classList.remove("open");
  $("drawer").setAttribute("aria-hidden", "true");
  if (!$("modal").classList.contains("show")) $("backdrop").classList.remove("show");
}

async function openCard(id) {
  try {
    const data = await api("/api/card", { id });
    $("drawer-title").textContent = `${t("details", "Картка")} #${id}`;
    $("drawer-subtitle").textContent = data.source_file || "";
    $("drawer-body").innerHTML = data.fields
      .map((field) => {
        const value = esc(field.value || "");
        const profile =
          field.label.includes("ЕДРПОУ") && field.value
            ? `<button class="profile-button" data-edrpou="${esc(field.value)}" type="button">${esc(t("company_profile", "Профіль"))}</button>`
            : "";
        return `<div class="field"><div class="field-label">${esc(field.label)}</div><div class="field-value">${value || "&nbsp;"}</div>${profile}</div>`;
      })
      .join("");
    openDrawer();
    for (const button of document.querySelectorAll(".profile-button")) {
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        openCompany(button.dataset.edrpou);
      });
    }
  } catch (err) {
    toast(err.message);
  }
}

async function openCompany(edrpou) {
  if (!edrpou) return;
  try {
    const data = await api("/api/company", { edrpou, limit: 10 });
    const profile = data.profile;
    $("drawer-title").textContent = `${t("company_profile", "Компанія")} ${profile.edrpou}`;
    $("drawer-subtitle").textContent = (profile.names || []).join(" · ");
    $("drawer-body").innerHTML = `
      <div class="kpi-grid drawer-kpis">${kpiHtml(profile.overview)}</div>
      ${sectionsHtml(profile.product_sections || [], false)}
      ${sectionsHtml(profile.country_sections || [], false)}
      ${pricesHtml(profile.price_sections || [])}`;
    openDrawer();
  } catch (err) {
    toast(err.message);
  }
}

/* ---------- analytics ---------- */
async function loadAnalytics() {
  const query = collectQuery();
  const scroll = $("analytics-scroll");
  if (queryIsEmpty(query)) {
    $("analytics-meta").textContent = t("analytics_hint", "—");
    scroll.innerHTML = `<div class="empty">${esc(t("analytics_hint", "—"))}</div>`;
    return;
  }
  $("analytics-meta").textContent = t("searching", "…");
  try {
    const data = await api("/api/analytics", {
      ...query,
      limit: $("analytics-limit").value,
      hs_level: 10,
    });
    if (data.needs_query) {
      $("analytics-meta").textContent = data.message;
      scroll.innerHTML = `<div class="empty">${esc(data.message)}</div>`;
      return;
    }
    const a = data.analytics;
    $("analytics-meta").textContent = fmt(t("search_ms", "in {} ms"), fmtInt.format(data.elapsed_ms));
    const grid = (label, inner) =>
      inner.trim()
        ? `<div class="analytics-grid">${inner}</div>`
        : `<div class="empty">${esc(label)}</div>`;
    scroll.innerHTML = `
      <div class="analytics-group" data-group="overview">
        <div class="kpi-grid">${kpiHtml(a.overview)}</div>
        <div class="block-title">${esc(t("months_section", "Динаміка за місяцями"))}</div>
        <div class="month-chart">${monthsHtml(a.months || [])}</div>
      </div>
      <div class="analytics-group" data-group="companies">${grid(t("nothing_found", "—"), sectionsHtml(a.company_sections || []))}</div>
      <div class="analytics-group" data-group="goods">${grid(t("nothing_found", "—"), sectionsHtml(a.product_sections || []))}</div>
      <div class="analytics-group" data-group="countries">${grid(t("nothing_found", "—"), sectionsHtml(a.country_sections || []))}</div>
      <div class="analytics-group" data-group="prices">${grid(t("nothing_found", "—"), pricesHtml(a.price_sections || []))}</div>`;
    applyAnalyticsGroup();
    bindAnalyticsClicks();
  } catch (err) {
    $("analytics-meta").textContent = err.message;
    scroll.innerHTML = `<div class="empty">${esc(err.message)}</div>`;
    toast(err.message);
  }
}

function applyAnalyticsGroup() {
  for (const node of document.querySelectorAll(".analytics-group")) {
    node.classList.toggle("active", node.dataset.group === state.analyticsGroup);
  }
  for (const chip of document.querySelectorAll("#analytics-subtabs .chip")) {
    chip.classList.toggle("active", chip.dataset.group === state.analyticsGroup);
  }
}

function kpiHtml(overview) {
  if (!overview) return "";
  const items = [
    [t("rows_label", "Рядки"), fmtInt.format(overview.row_count)],
    [t("declarations_label", "Декларації"), fmtInt.format(overview.declaration_count)],
    [t("recipients_label", "Одержувачі"), fmtInt.format(overview.distinct_recipients)],
    [t("unique_senders", "Відправники"), fmtInt.format(overview.distinct_senders)],
    [t("total_value", "Сума"), `${fmtNum.format(overview.total_value_usd)} $`],
    [t("net_weight", "Нетто"), `${fmtKg.format(overview.total_net_kg)}`],
    [t("gross_weight", "Брутто"), `${fmtKg.format(overview.total_gross_kg)}`],
    [t("avg_value_kg", "$/кг"), fmtNum.format(overview.avg_value_per_net_kg)],
  ];
  return items
    .map(
      ([label, value]) =>
        `<div class="kpi"><div class="label">${esc(label)}</div><div class="value">${esc(value)}</div></div>`,
    )
    .join("");
}

function monthsHtml(months) {
  if (!months.length) return `<div class="empty">${esc(t("nothing_found", "—"))}</div>`;
  const max = Math.max(...months.map((m) => Number(m.total_value_usd) || Number(m.rows) || 0), 1);
  return months
    .map((month) => {
      const value = Number(month.total_value_usd) || Number(month.rows) || 0;
      const height = Math.max(3, Math.round((value / max) * 100));
      return `<div class="month-bar" title="${esc(month.month)}: ${fmtNum.format(value)} $">
        <i style="height:${height}%"></i><small>${esc(month.month.slice(2))}</small>
      </div>`;
    })
    .join("");
}

function sectionsHtml(sections, allowAll = true) {
  return sections
    .filter((section) => section.rows && section.rows.length)
    .map((section) => {
      const title = sectionTitle(section.kind, section.title);
      const allButton =
        allowAll && section.kind
          ? `<button class="all-btn" type="button" data-kind="${esc(section.kind)}" data-title="${esc(title)}">${esc(t("all_label", "Усі"))}</button>`
          : "";
      const rows = section.rows
        .map((row) => {
          const width = Math.max(2, Math.min(100, Number(row.share_percent) || 0));
          const action = row.filter_action
            ? ` data-field="${esc(row.filter_action.field)}" data-value="${esc(row.filter_action.value)}"`
            : "";
          return `<div class="rank-row"${action}>
            <div>
              <div class="rank-name">${esc(row.label || "—")}</div>
              <div class="bar-track"><div class="bar" style="width:${width}%"></div></div>
            </div>
            <div class="rank-stats">
              <b>${fmtNum.format(row.total_value_usd)} $</b><br />
              ${fmtKg.format(row.total_net_kg)} · ${fmtInt.format(row.rows)}
            </div>
          </div>`;
        })
        .join("");
      return `<section class="section"><div class="section-head"><h3>${esc(title)}</h3>${allButton}</div>${rows}</section>`;
    })
    .join("");
}

function pricesHtml(metrics) {
  const rows = (metrics || [])
    .filter((metric) => metric.count > 0)
    .map(
      (metric) => `<div class="rank-row static">
        <div class="rank-name">${esc(priceTitle(metric.kind, metric.title))}</div>
        <div class="rank-stats">
          ${esc(t("median", "медіана"))} <b>${fmtNum.format(metric.median)}</b><br />
          ${fmtNum.format(metric.average)} · ${fmtInt.format(metric.count)}
        </div>
      </div>`,
    )
    .join("");
  return rows ? `<section class="section"><div class="section-head"><h3>${esc(t("prices_section", "Ціни"))}</h3></div>${rows}</section>` : "";
}

function applyFilterAction(field, value) {
  if (!field || !$(field)) return;
  $(field).value = value;
  activateTab("results");
  search(true);
}

function bindAnalyticsClicks() {
  for (const row of document.querySelectorAll(".rank-row[data-field]")) {
    row.addEventListener("click", () => applyFilterAction(row.dataset.field, row.dataset.value));
  }
  for (const button of document.querySelectorAll(".all-btn[data-kind]")) {
    button.addEventListener("click", () => openSection(button.dataset.kind, button.dataset.title));
  }
}

/* ---------- drill-down modal (show all groups) ---------- */
function sectionColumns() {
  return [
    { key: "label", label: t("col_label", "Назва"), text: true },
    { key: "rows", label: t("rows_label", "Рядків"), fmt: (v) => fmtInt.format(v) },
    { key: "declarations", label: t("declarations_label", "Декларацій"), fmt: (v) => fmtInt.format(v) },
    { key: "companies", label: t("col_companies", "Компаній"), fmt: (v) => fmtInt.format(v) },
    { key: "total_value_usd", label: t("total_value", "Сума"), fmt: (v) => fmtNum.format(v) },
    { key: "total_net_kg", label: t("net_weight", "Нетто"), fmt: (v) => fmtKg.format(v) },
    { key: "share_percent", label: t("col_share", "Частка"), fmt: (v) => `${fmtNum.format(v)}%` },
    { key: "avg_value_kg", label: t("avg_value_kg", "$/кг"), src: "avg_value_per_net_kg", fmt: (v) => fmtNum.format(v) },
  ];
}

function visibleSectionRows() {
  const s = state.section;
  const needle = s.filter.trim().toLowerCase();
  let rows = needle
    ? s.rows.filter((r) => String(r.label).toLowerCase().includes(needle))
    : s.rows.slice();
  const key = s.sort === "avg_value_kg" ? "avg_value_per_net_kg" : s.sort;
  rows.sort((a, b) => {
    if (s.sort === "label") {
      const av = String(a.label).toLowerCase();
      const bv = String(b.label).toLowerCase();
      return s.desc ? bv.localeCompare(av) : av.localeCompare(bv);
    }
    const av = Number(a[key]) || 0;
    const bv = Number(b[key]) || 0;
    return s.desc ? bv - av : av - bv;
  });
  return rows;
}

function renderSection() {
  const s = state.section;
  if (!s) return;
  const cols = sectionColumns();
  const rows = visibleSectionRows();
  const head = document.querySelector("#modal-table thead");
  head.innerHTML =
    "<tr>" +
    cols
      .map((c) => {
        const arrow = c.key === s.sort ? ` <span class="arrow">${s.desc ? "▼" : "▲"}</span>` : "";
        return `<th${c.text ? "" : ' class="num"'} data-key="${c.key}">${esc(c.label)}${arrow}</th>`;
      })
      .join("") +
    "</tr>";
  const body = document.querySelector("#modal-table tbody");
  body.innerHTML = rows
    .map((r) => {
      const action = r.filter_action
        ? ` data-field="${esc(r.filter_action.field)}" data-value="${esc(r.filter_action.value)}"`
        : "";
      const cells = cols
        .map((c) => {
          if (c.text) return `<td title="${esc(r.label)}">${esc(r.label)}</td>`;
          const raw = r[c.src || c.key];
          return `<td class="num">${c.fmt(raw)}</td>`;
        })
        .join("");
      return `<tr${action}>${cells}</tr>`;
    })
    .join("");
  $("modal-meta").textContent =
    fmt(t("showing", "{} / {}"), fmtInt.format(rows.length), fmtInt.format(s.rows.length)) +
    (s.limited ? " · max" : "");
  for (const th of head.querySelectorAll("th")) {
    th.addEventListener("click", () => {
      const key = th.dataset.key;
      if (s.sort === key) s.desc = !s.desc;
      else {
        s.sort = key;
        s.desc = key !== "label";
      }
      renderSection();
    });
  }
  for (const tr of body.querySelectorAll("tr[data-field]")) {
    tr.addEventListener("click", () => {
      applyFilterAction(tr.dataset.field, tr.dataset.value);
      closeModal();
    });
  }
}

async function openSection(kind, title) {
  state.section = { kind, title, rows: [], sort: "total_value_usd", desc: true, filter: "", limited: false };
  $("modal-title").textContent = title || t("all_label", "Усі");
  $("modal-search").value = "";
  $("modal-search").placeholder = t("group_search_hint", "Пошук у списку");
  $("modal-meta").textContent = t("searching", "…");
  document.querySelector("#modal-table thead").innerHTML = "";
  document.querySelector("#modal-table tbody").innerHTML = "";
  openModal();
  try {
    const data = await api("/api/section", { ...collectQuery(), kind, hs_level: 10, limit: 20000 });
    if (data.needs_query) {
      $("modal-meta").textContent = data.message;
      return;
    }
    state.section.rows = data.rows || [];
    state.section.limited = Boolean(data.limited);
    renderSection();
  } catch (err) {
    $("modal-meta").textContent = err.message;
    toast(err.message);
  }
}

function copySectionVisible() {
  if (!state.section) return;
  const cols = sectionColumns();
  const rows = visibleSectionRows();
  const header = cols.map((c) => c.label).join("\t");
  const lines = rows.map((r) =>
    cols
      .map((c) => (c.text ? r.label : r[c.src || c.key]))
      .join("\t"),
  );
  navigator.clipboard
    .writeText([header, ...lines].join("\n"))
    .then(() => toast("OK"))
    .catch(() => toast("clipboard error"));
}

function openModal() {
  $("modal").classList.add("show");
  $("modal").setAttribute("aria-hidden", "false");
}

function closeModal() {
  $("modal").classList.remove("show");
  $("modal").setAttribute("aria-hidden", "true");
}

/* ---------- tabs / actions ---------- */
function activateTab(tab) {
  state.activeTab = tab;
  for (const button of document.querySelectorAll(".tab")) {
    button.classList.toggle("active", button.dataset.tab === tab);
  }
  $("results-panel").classList.toggle("active", tab === "results");
  $("analytics-panel").classList.toggle("active", tab === "analytics");
  if (tab === "analytics") loadAnalytics();
}

function exportCurrentPage() {
  if (!state.token) {
    toast("Немає локального токена сесії");
    return;
  }
  const params = new URLSearchParams(cleanParams({ ...queryWithPaging(), token: state.token }));
  location.href = `/api/export-page.csv?${params.toString()}`;
}

function clearFilters() {
  for (const id of fieldIds) {
    if (id !== "text") $(id).value = "";
  }
  search(true);
}

function bindEvents() {
  $("search-form").addEventListener("submit", (event) => {
    event.preventDefault();
    search(true);
  });
  for (const id of fieldIds) {
    if (id === "text") continue;
    $(id).addEventListener("keydown", (event) => {
      if (event.key === "Enter") search(true);
    });
  }
  $("refresh").addEventListener("click", () => {
    loadStats();
    search(false);
  });
  $("export-page").addEventListener("click", exportCurrentPage);
  $("clear-filters").addEventListener("click", clearFilters);
  $("lang-select").addEventListener("change", onLangChange);
  $("prev-page").addEventListener("click", () => {
    if (state.page > 0) {
      state.page -= 1;
      search(false);
    }
  });
  $("next-page").addEventListener("click", () => {
    if (state.hasNext) {
      state.page += 1;
      search(false);
    }
  });
  $("drawer-close").addEventListener("click", closeDrawer);
  $("backdrop").addEventListener("click", closeDrawer);
  $("modal-close").addEventListener("click", closeModal);
  $("modal").addEventListener("click", (event) => {
    if (event.target === $("modal")) closeModal();
  });
  $("modal-copy").addEventListener("click", copySectionVisible);
  $("modal-search").addEventListener("input", (event) => {
    if (state.section) {
      state.section.filter = event.target.value;
      renderSection();
    }
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    if ($("modal").classList.contains("show")) closeModal();
    else closeDrawer();
  });
  $("analytics-limit").addEventListener("change", loadAnalytics);
  for (const button of document.querySelectorAll(".tab")) {
    button.addEventListener("click", () => activateTab(button.dataset.tab));
  }
  for (const chip of document.querySelectorAll("#analytics-subtabs .chip")) {
    chip.addEventListener("click", () => {
      state.analyticsGroup = chip.dataset.group;
      applyAnalyticsGroup();
    });
  }
}

async function boot() {
  bindEvents();
  if (!state.token) {
    toast("Запустіть браузерний режим через BaseSearch.exe --web");
    return;
  }
  try {
    await loadI18n();
    await loadSchema();
    await loadStats();
    await search(true);
  } catch (err) {
    toast(err.message);
  }
}

boot();
