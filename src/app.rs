//! Graphical interface: search bar, filters, paginated table, record card,
//! import/export progress, and settings.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui_extras::{Column, TableBuilder};

use crate::db::{
    Analytics, AnalyticsFilterAction, AnalyticsFilterField, AnalyticsGroupRow, AnalyticsMonthRow,
    AnalyticsPriceMetric, AnalyticsScope, AnalyticsSection, AnalyticsSectionKind, CompanyProfile,
    Db, Filters, PivotDim, PivotMetric, PivotResult, PriceMetricKind, Query, RecordCard,
    Undervaluation, pivot_filter_action,
};
use crate::export::ExportError;
use crate::i18n::{Lang, Tr, fmt, group_digits, help_sections, tr};
use crate::import::{FileSummary, ImportPhase};
use crate::schema::{RESULT_COLUMNS, column_glossary, header_for};
use crate::workers::{self, ImportEvent, Msg, PAGE_SIZE, WorkerReq};

/// Interface accent color.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(37, 99, 235);
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const FULL_SECTION_LIMIT: u64 = 20_000;
const RECENT_QUERIES_META: &str = "recent_queries_v1";
const SAVED_QUERIES_META: &str = "saved_queries_v1";
const RECENT_QUERY_LIMIT: usize = 12;

/// Action from the table row context menu.
enum RowMenuAction {
    CopyCell(String),
    CopyRow(usize),
    CopySelected,
    FilterSender(String),
    FilterRecipient(String),
    FilterCode(String),
    FilterEdrpou(String),
    OpenProfile(String),
}

type QuickAction = (&'static str, &'static str, fn(String) -> RowMenuAction);

/// Visual cell type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    /// Primary text, such as descriptions and companies.
    Normal,
    /// Secondary text, such as dates, countries, and organization codes.
    Weak,
    /// Product code: monospace and accented.
    Code,
    /// Numbers: monospace and right-aligned.
    Number,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppTab {
    Results,
    Analytics,
}

enum AnalyticsCardAction {
    Filter(AnalyticsFilterAction),
    Explore(AnalyticsSectionKind),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GuidedQuestionSection {
    Product,
    Company,
    Market,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GuidedQuestionKind {
    ProductCompanies,
    ProductAllCompanies,
    ProductGoods,
    ProductCountries,
    ProductPrices,
    ProductTimeline,
    ProductCompaniesByMonth,
    CompanyProfile,
    CompanyGoods,
    CompanySuppliers,
    CompanyCountries,
    CompanyTimeline,
    CompanyGoodsByMonth,
    MarketCompanies,
    MarketGoods,
    MarketCountries,
    MarketPrices,
}

enum GuidedQuestionAction {
    Analytics(AnalyticsView),
    Explore(AnalyticsSectionKind),
    Pivot(PivotDim, PivotDim, PivotMetric),
    Profile(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupSort {
    Label,
    Rows,
    Declarations,
    Companies,
    Value,
    NetKg,
    GrossKg,
    Quantity,
    Share,
    ValuePerKg,
}

struct GroupExplorerState {
    kind: AnalyticsSectionKind,
    generation: u64,
    loading: bool,
    rows: Vec<AnalyticsGroupRow>,
    label_filter: String,
    sort: GroupSort,
    descending: bool,
}

/// Metric displayed in the monthly dynamics chart.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum MonthMetric {
    #[default]
    Value,
    Rows,
    NetWeight,
    /// Monthly average price: value / net weight.
    AvgPrice,
}

impl MonthMetric {
    fn of(self, row: &AnalyticsMonthRow) -> f64 {
        match self {
            MonthMetric::Value => row.total_value_usd,
            MonthMetric::Rows => row.rows as f64,
            MonthMetric::NetWeight => row.total_net_kg,
            MonthMetric::AvgPrice => {
                if row.total_net_kg > 0.0 {
                    row.total_value_usd / row.total_net_kg
                } else {
                    0.0
                }
            }
        }
    }
}

/// Sub-tab of the Analytics view: Overview, four data categories, and the
/// cross-tab (pivot).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum AnalyticsView {
    #[default]
    Overview,
    Companies,
    Products,
    Countries,
    Prices,
    Pivot,
}

impl AnalyticsView {
    const ALL: [AnalyticsView; 6] = [
        AnalyticsView::Overview,
        AnalyticsView::Companies,
        AnalyticsView::Products,
        AnalyticsView::Countries,
        AnalyticsView::Prices,
        AnalyticsView::Pivot,
    ];

    fn index(self) -> usize {
        match self {
            AnalyticsView::Overview => 0,
            AnalyticsView::Companies => 1,
            AnalyticsView::Products => 2,
            AnalyticsView::Countries => 3,
            AnalyticsView::Prices => 4,
            AnalyticsView::Pivot => 5,
        }
    }

    /// Section scope for the standard sub-tabs; Overview and Pivot have none.
    fn scope(self) -> Option<AnalyticsScope> {
        match self {
            AnalyticsView::Companies => Some(AnalyticsScope::Companies),
            AnalyticsView::Products => Some(AnalyticsScope::Products),
            AnalyticsView::Countries => Some(AnalyticsScope::Countries),
            AnalyticsView::Prices => Some(AnalyticsScope::Prices),
            AnalyticsView::Overview | AnalyticsView::Pivot => None,
        }
    }

    fn from_scope(scope: Option<AnalyticsScope>) -> AnalyticsView {
        match scope {
            None => AnalyticsView::Overview,
            Some(AnalyticsScope::Companies) => AnalyticsView::Companies,
            Some(AnalyticsScope::Products) => AnalyticsView::Products,
            Some(AnalyticsScope::Countries) => AnalyticsView::Countries,
            Some(AnalyticsScope::Prices) => AnalyticsView::Prices,
        }
    }
}

/// Result column width and visual style.
fn col_spec(name: &str) -> (f32, CellKind) {
    match name {
        "clearance_time" => (130.0, CellKind::Weak),
        "customs_office" => (190.0, CellKind::Weak),
        "declaration_type" => (72.0, CellKind::Weak),
        "declaration_date" => (88.0, CellKind::Weak),
        "declaration_number" => (150.0, CellKind::Weak),
        "sender" => (195.0, CellKind::Normal),
        "recipient" => (195.0, CellKind::Normal),
        "item_number" => (58.0, CellKind::Number),
        "description" => (440.0, CellKind::Normal),
        "product_code" => (104.0, CellKind::Code),
        "edrpou" => (88.0, CellKind::Weak),
        "trade_country" | "dispatch_country" | "origin_country" => (76.0, CellKind::Weak),
        "delivery_terms" => (92.0, CellKind::Weak),
        "delivery_place" => (140.0, CellKind::Weak),
        "quantity" => (76.0, CellKind::Number),
        "unit" => (72.0, CellKind::Weak),
        "gross_kg"
        | "net_kg"
        | "declaration_weight"
        | "currency_control_value"
        | "rfv_usd_kg"
        | "unit_weight"
        | "weight_difference"
        | "rmv_net_usd_kg"
        | "rmv_usd_extra_unit"
        | "rmv_gross_usd_kg"
        | "min_base_usd_kg"
        | "min_base_difference"
        | "preferential"
        | "full_rate" => (112.0, CellKind::Number),
        "contract" => (150.0, CellKind::Weak),
        "trademark" => (110.0, CellKind::Weak),
        "source_file" => (140.0, CellKind::Weak),
        _ => (110.0, CellKind::Normal),
    }
}

fn trunc_label(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('\u{2026}');
    }
    out
}

fn query_summary(query: &Query, t: &Tr) -> String {
    if query.is_empty() {
        return t.enter_query_hint.to_string();
    }
    let f = &query.filters;
    let mut parts = Vec::new();
    if !query.text.trim().is_empty() {
        parts.push(query.text.trim().to_string());
    }
    for (label, value) in [
        (t.year, &f.year),
        (t.product_code, &f.product_code),
        (t.trademark, &f.trademark),
        (t.description, &f.description),
        (t.sender, &f.sender),
        (t.recipient, &f.recipient),
        (t.edrpou, &f.edrpou),
        (t.trade_country, &f.trade_country),
        (t.dispatch_country, &f.dispatch_country),
        (t.origin_country, &f.origin_country),
    ] {
        let value = value.trim();
        if !value.is_empty() {
            parts.push(format!("{label}: {value}"));
        }
    }
    parts.join(" · ")
}

fn encode_stored_queries(items: &[StoredQuery]) -> String {
    items
        .iter()
        .map(|item| {
            let f = &item.query.filters;
            [
                item.name.as_str(),
                item.query.text.as_str(),
                f.year.as_str(),
                f.product_code.as_str(),
                f.trademark.as_str(),
                f.description.as_str(),
                f.sender.as_str(),
                f.recipient.as_str(),
                f.edrpou.as_str(),
                f.trade_country.as_str(),
                f.dispatch_country.as_str(),
                f.origin_country.as_str(),
            ]
            .iter()
            .map(|value| encode_component(value))
            .collect::<Vec<_>>()
            .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_stored_queries(raw: &str) -> Vec<StoredQuery> {
    raw.lines()
        .filter_map(|line| {
            let fields = line
                .split('\t')
                .map(decode_component)
                .collect::<Option<Vec<_>>>()?;
            if fields.len() != 12 {
                return None;
            }
            let query = Query {
                text: fields[1].clone(),
                filters: Filters {
                    year: fields[2].clone(),
                    product_code: fields[3].clone(),
                    trademark: fields[4].clone(),
                    description: fields[5].clone(),
                    sender: fields[6].clone(),
                    recipient: fields[7].clone(),
                    edrpou: fields[8].clone(),
                    trade_country: fields[9].clone(),
                    dispatch_country: fields[10].clone(),
                    origin_country: fields[11].clone(),
                },
            };
            if query.is_empty() {
                return None;
            }
            let name = if fields[0].trim().is_empty() {
                query_summary(&query, tr(Lang::En))
            } else {
                fields[0].clone()
            };
            Some(StoredQuery { name, query })
        })
        .collect()
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            _ => out.push(ch),
        }
    }
    out
}

fn decode_component(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = &value[i + 1..i + 3];
            match hex {
                "25" => out.push('%'),
                "09" => out.push('\t'),
                "0A" => out.push('\n'),
                "0D" => out.push('\r'),
                _ => return None,
            }
            i += 3;
        } else {
            let ch = value[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Some(out)
}

fn recent_queries_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Недавні запити",
        Lang::Ru => "Недавние запросы",
        _ => "Recent searches",
    }
}

fn saved_queries_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Збережені запити",
        Lang::Ru => "Сохранённые запросы",
        _ => "Saved searches",
    }
}

fn save_current_query_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Зберегти поточний запит",
        Lang::Ru => "Сохранить текущий запрос",
        _ => "Save current search",
    }
}

fn empty_recent_queries_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Недавніх запитів ще немає",
        Lang::Ru => "Недавних запросов пока нет",
        _ => "No recent searches yet",
    }
}

fn empty_saved_queries_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Збережених запитів ще немає",
        Lang::Ru => "Сохранённых запросов пока нет",
        _ => "No saved searches yet",
    }
}

fn clear_recent_queries_label(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Очистити історію",
        Lang::Ru => "Очистить историю",
        _ => "Clear history",
    }
}

fn guided_questions_label(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Questions",
        Lang::Ua => "Питання",
        Lang::Ru => "Вопросы",
        Lang::De => "Fragen",
        Lang::Es => "Preguntas",
        Lang::Fr => "Questions",
        Lang::Pl => "Pytania",
        Lang::Pt => "Perguntas",
        Lang::Ro => "Întrebări",
        Lang::Hu => "Kérdések",
        Lang::Bg => "Въпроси",
        Lang::Be => "Пытанні",
        Lang::Zh => "问题",
    }
}

fn guided_questions_hover(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Smart shortcuts for the current product, company, or filtered slice.",
        Lang::Ua => "Розумні сценарії для поточного товару, компанії або фільтра.",
        Lang::Ru => "Умные сценарии для текущего товара, компании или фильтра.",
        Lang::De => "Intelligente Wege für Ware, Firma oder aktuellen Filter.",
        Lang::Es => "Atajos inteligentes para el producto, empresa o filtro actual.",
        Lang::Fr => "Raccourcis intelligents pour le produit, l'entreprise ou le filtre actuel.",
        Lang::Pl => "Inteligentne skróty dla towaru, firmy albo bieżącego filtra.",
        Lang::Pt => "Atalhos inteligentes para produto, empresa ou filtro atual.",
        Lang::Ro => "Scurtături inteligente pentru produs, companie sau filtrul curent.",
        Lang::Hu => "Okos útvonalak az aktuális termékhez, céghez vagy szűrőhöz.",
        Lang::Bg => "Умни преки пътища за текущия продукт, фирма или филтър.",
        Lang::Be => "Разумныя сцэнарыі для тавару, кампаніі або бягучага фільтра.",
        Lang::Zh => "当前商品、公司或筛选范围的智能分析入口。",
    }
}

fn guided_questions_empty(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Enter a product, company, code, year, or country first.",
        Lang::Ua => "Спочатку введіть товар, компанію, код, рік або країну.",
        Lang::Ru => "Сначала введите товар, компанию, код, год или страну.",
        Lang::De => "Geben Sie zuerst Ware, Firma, Code, Jahr oder Land ein.",
        Lang::Es => "Primero introduzca producto, empresa, código, año o país.",
        Lang::Fr => "Saisissez d'abord un produit, une entreprise, un code, une année ou un pays.",
        Lang::Pl => "Najpierw wpisz towar, firmę, kod, rok albo kraj.",
        Lang::Pt => "Digite primeiro produto, empresa, código, ano ou país.",
        Lang::Ro => "Introduceți mai întâi produs, companie, cod, an sau țară.",
        Lang::Hu => "Először adjon meg terméket, céget, kódot, évet vagy országot.",
        Lang::Bg => "Първо въведете продукт, фирма, код, година или държава.",
        Lang::Be => "Спачатку ўвядзіце тавар, кампанію, код, год або краіну.",
        Lang::Zh => "请先输入商品、公司、编码、年份或国家。",
    }
}

fn guided_section_title(section: GuidedQuestionSection, lang: Lang) -> &'static str {
    match section {
        GuidedQuestionSection::Product => match lang {
            Lang::En => "For this product or search",
            Lang::Ua => "Для цього товару або запиту",
            Lang::Ru => "По этому товару или запросу",
            Lang::De => "Zu dieser Ware oder Suche",
            Lang::Es => "Para este producto o búsqueda",
            Lang::Fr => "Pour ce produit ou cette recherche",
            Lang::Pl => "Dla tego towaru lub wyszukiwania",
            Lang::Pt => "Para este produto ou busca",
            Lang::Ro => "Pentru acest produs sau căutare",
            Lang::Hu => "Ehhez a termékhez vagy kereséshez",
            Lang::Bg => "За този продукт или търсене",
            Lang::Be => "Для гэтага тавару або запыту",
            Lang::Zh => "针对当前商品或搜索",
        },
        GuidedQuestionSection::Company => match lang {
            Lang::En => "For this company",
            Lang::Ua => "Для цієї компанії",
            Lang::Ru => "По этой компании",
            Lang::De => "Zu dieser Firma",
            Lang::Es => "Para esta empresa",
            Lang::Fr => "Pour cette entreprise",
            Lang::Pl => "Dla tej firmy",
            Lang::Pt => "Para esta empresa",
            Lang::Ro => "Pentru această companie",
            Lang::Hu => "Ehhez a céghez",
            Lang::Bg => "За тази фирма",
            Lang::Be => "Для гэтай кампаніі",
            Lang::Zh => "针对当前公司",
        },
        GuidedQuestionSection::Market => match lang {
            Lang::En => "For the current slice",
            Lang::Ua => "Для поточної вибірки",
            Lang::Ru => "По текущей выборке",
            Lang::De => "Zum aktuellen Ausschnitt",
            Lang::Es => "Para la selección actual",
            Lang::Fr => "Pour le périmètre actuel",
            Lang::Pl => "Dla bieżącego zakresu",
            Lang::Pt => "Para o recorte atual",
            Lang::Ro => "Pentru selecția curentă",
            Lang::Hu => "Az aktuális szűréshez",
            Lang::Bg => "За текущата извадка",
            Lang::Be => "Для бягучай выбаркі",
            Lang::Zh => "针对当前筛选范围",
        },
    }
}

fn guided_question_title(kind: GuidedQuestionKind, lang: Lang) -> &'static str {
    match kind {
        GuidedQuestionKind::ProductCompanies => match lang {
            Lang::En => "Who received or imported it?",
            Lang::Ua => "Хто отримував або ввозив це?",
            Lang::Ru => "Кто получал или ввозил это?",
            Lang::De => "Wer hat es erhalten oder importiert?",
            Lang::Es => "¿Quién lo recibió o importó?",
            Lang::Fr => "Qui l'a reçu ou importé ?",
            Lang::Pl => "Kto to odbierał lub importował?",
            Lang::Pt => "Quem recebeu ou importou isso?",
            Lang::Ro => "Cine a primit sau importat?",
            Lang::Hu => "Ki kapta vagy importálta?",
            Lang::Bg => "Кой го е получавал или внасял?",
            Lang::Be => "Хто атрымліваў або ўвозіў гэта?",
            Lang::Zh => "谁接收或进口了它？",
        },
        GuidedQuestionKind::ProductAllCompanies => match lang {
            Lang::En => "Show every company and EDRPOU",
            Lang::Ua => "Показати всі компанії та ЄДРПОУ",
            Lang::Ru => "Показать все компании и ЕДРПОУ",
            Lang::De => "Alle Firmen und EDRPOU anzeigen",
            Lang::Es => "Mostrar todas las empresas y EDRPOU",
            Lang::Fr => "Afficher toutes les entreprises et EDRPOU",
            Lang::Pl => "Pokaż wszystkie firmy i EDRPOU",
            Lang::Pt => "Mostrar todas as empresas e EDRPOU",
            Lang::Ro => "Arată toate companiile și EDRPOU",
            Lang::Hu => "Összes cég és EDRPOU megjelenítése",
            Lang::Bg => "Покажи всички фирми и ЕДРПОУ",
            Lang::Be => "Паказаць усе кампаніі і ЕДРПОУ",
            Lang::Zh => "显示所有公司和EDRPOU",
        },
        GuidedQuestionKind::ProductGoods => match lang {
            Lang::En => "Which product codes and brands are inside?",
            Lang::Ua => "Які коди товару та бренди всередині?",
            Lang::Ru => "Какие коды товара и бренды внутри?",
            Lang::De => "Welche Warencodes und Marken stecken darin?",
            Lang::Es => "¿Qué códigos y marcas contiene?",
            Lang::Fr => "Quels codes produit et marques contient-il ?",
            Lang::Pl => "Jakie kody towaru i marki są w środku?",
            Lang::Pt => "Quais códigos de produto e marcas aparecem?",
            Lang::Ro => "Ce coduri de produs și mărci conține?",
            Lang::Hu => "Milyen termékkódok és márkák vannak benne?",
            Lang::Bg => "Какви кодове и марки има вътре?",
            Lang::Be => "Якія коды тавару і брэнды ўнутры?",
            Lang::Zh => "包含哪些商品编码和品牌？",
        },
        GuidedQuestionKind::ProductCountries => match lang {
            Lang::En => "From which countries and routes?",
            Lang::Ua => "З яких країн і маршрутів?",
            Lang::Ru => "Из каких стран и маршрутов?",
            Lang::De => "Aus welchen Ländern und Routen?",
            Lang::Es => "¿Desde qué países y rutas?",
            Lang::Fr => "Depuis quels pays et routes ?",
            Lang::Pl => "Z jakich krajów i tras?",
            Lang::Pt => "De quais países e rotas?",
            Lang::Ro => "Din ce țări și rute?",
            Lang::Hu => "Mely országokból és útvonalakon?",
            Lang::Bg => "От кои държави и маршрути?",
            Lang::Be => "З якіх краін і маршрутаў?",
            Lang::Zh => "来自哪些国家和路线？",
        },
        GuidedQuestionKind::ProductPrices => match lang {
            Lang::En => "What is the price and $/kg picture?",
            Lang::Ua => "Яка ціна та картина $/кг?",
            Lang::Ru => "Какая картина по цене и $/кг?",
            Lang::De => "Wie sieht Preis und $/kg aus?",
            Lang::Es => "¿Cómo se ven precio y $/kg?",
            Lang::Fr => "Quelle est la situation prix et $/kg ?",
            Lang::Pl => "Jak wygląda cena i $/kg?",
            Lang::Pt => "Como estão preço e $/kg?",
            Lang::Ro => "Cum arată prețul și $/kg?",
            Lang::Hu => "Milyen az ár és $/kg képe?",
            Lang::Bg => "Как изглеждат цена и $/кг?",
            Lang::Be => "Якая карціна па цане і $/кг?",
            Lang::Zh => "价格和$/公斤情况如何？",
        },
        GuidedQuestionKind::ProductTimeline => match lang {
            Lang::En => "How did value and weight change by month?",
            Lang::Ua => "Як змінювались вартість і вага по місяцях?",
            Lang::Ru => "Как менялись сумма и вес по месяцам?",
            Lang::De => "Wie änderten sich Wert und Gewicht je Monat?",
            Lang::Es => "¿Cómo cambiaron valor y peso por mes?",
            Lang::Fr => "Comment valeur et poids ont-ils changé par mois ?",
            Lang::Pl => "Jak zmieniały się wartość i waga miesięcznie?",
            Lang::Pt => "Como valor e peso mudaram por mês?",
            Lang::Ro => "Cum s-au schimbat valoarea și greutatea lunar?",
            Lang::Hu => "Hogyan változott az érték és a súly havonta?",
            Lang::Bg => "Как се променяха стойност и тегло по месеци?",
            Lang::Be => "Як змяняліся сума і вага па месяцах?",
            Lang::Zh => "金额和重量按月如何变化？",
        },
        GuidedQuestionKind::ProductCompaniesByMonth => match lang {
            Lang::En => "Compare companies by month",
            Lang::Ua => "Порівняти компанії по місяцях",
            Lang::Ru => "Сравнить компании по месяцам",
            Lang::De => "Firmen nach Monaten vergleichen",
            Lang::Es => "Comparar empresas por mes",
            Lang::Fr => "Comparer les entreprises par mois",
            Lang::Pl => "Porównaj firmy według miesięcy",
            Lang::Pt => "Comparar empresas por mês",
            Lang::Ro => "Compară companiile pe luni",
            Lang::Hu => "Cégek összehasonlítása hónaponként",
            Lang::Bg => "Сравни фирмите по месеци",
            Lang::Be => "Параўнаць кампаніі па месяцах",
            Lang::Zh => "按月比较公司",
        },
        GuidedQuestionKind::CompanyProfile => match lang {
            Lang::En => "Open the full company dossier",
            Lang::Ua => "Відкрити повне досьє компанії",
            Lang::Ru => "Открыть полное досье компании",
            Lang::De => "Vollständiges Firmendossier öffnen",
            Lang::Es => "Abrir el expediente completo de la empresa",
            Lang::Fr => "Ouvrir le dossier complet de l'entreprise",
            Lang::Pl => "Otwórz pełny profil firmy",
            Lang::Pt => "Abrir o dossiê completo da empresa",
            Lang::Ro => "Deschide dosarul complet al companiei",
            Lang::Hu => "Teljes cégdosszié megnyitása",
            Lang::Bg => "Отвори пълното досие на фирмата",
            Lang::Be => "Адкрыць поўнае дасье кампаніі",
            Lang::Zh => "打开完整公司档案",
        },
        GuidedQuestionKind::CompanyGoods => match lang {
            Lang::En => "What did this company move?",
            Lang::Ua => "Що переміщувала ця компанія?",
            Lang::Ru => "Что возила эта компания?",
            Lang::De => "Welche Waren bewegte diese Firma?",
            Lang::Es => "¿Qué movió esta empresa?",
            Lang::Fr => "Quelles marchandises cette entreprise a-t-elle traitées ?",
            Lang::Pl => "Co przewoziła ta firma?",
            Lang::Pt => "O que esta empresa movimentou?",
            Lang::Ro => "Ce a transportat această companie?",
            Lang::Hu => "Mit mozgatott ez a cég?",
            Lang::Bg => "Какво е превозвала тази фирма?",
            Lang::Be => "Што вазіла гэта кампанія?",
            Lang::Zh => "这家公司运输了什么？",
        },
        GuidedQuestionKind::CompanySuppliers => match lang {
            Lang::En => "Who supplied this company?",
            Lang::Ua => "Хто постачав цій компанії?",
            Lang::Ru => "Кто поставлял этой компании?",
            Lang::De => "Wer belieferte diese Firma?",
            Lang::Es => "¿Quién abasteció a esta empresa?",
            Lang::Fr => "Qui a fourni cette entreprise ?",
            Lang::Pl => "Kto dostarczał tej firmie?",
            Lang::Pt => "Quem forneceu para esta empresa?",
            Lang::Ro => "Cine a furnizat această companie?",
            Lang::Hu => "Ki szállított ennek a cégnek?",
            Lang::Bg => "Кой е доставял на тази фирма?",
            Lang::Be => "Хто пастаўляў гэтай кампаніі?",
            Lang::Zh => "谁给这家公司供货？",
        },
        GuidedQuestionKind::CompanyCountries => match lang {
            Lang::En => "Which countries did it work with?",
            Lang::Ua => "З якими країнами вона працювала?",
            Lang::Ru => "С какими странами она работала?",
            Lang::De => "Mit welchen Ländern arbeitete sie?",
            Lang::Es => "¿Con qué países trabajó?",
            Lang::Fr => "Avec quels pays a-t-elle travaillé ?",
            Lang::Pl => "Z jakimi krajami współpracowała?",
            Lang::Pt => "Com quais países trabalhou?",
            Lang::Ro => "Cu ce țări a lucrat?",
            Lang::Hu => "Mely országokkal dolgozott?",
            Lang::Bg => "С кои държави е работила?",
            Lang::Be => "З якімі краінамі яна працавала?",
            Lang::Zh => "它与哪些国家往来？",
        },
        GuidedQuestionKind::CompanyTimeline => match lang {
            Lang::En => "How did this company change by month?",
            Lang::Ua => "Як ця компанія змінювалась по місяцях?",
            Lang::Ru => "Как эта компания менялась по месяцам?",
            Lang::De => "Wie veränderte sich diese Firma je Monat?",
            Lang::Es => "¿Cómo cambió esta empresa por mes?",
            Lang::Fr => "Comment cette entreprise a-t-elle évolué par mois ?",
            Lang::Pl => "Jak firma zmieniała się miesięcznie?",
            Lang::Pt => "Como esta empresa mudou por mês?",
            Lang::Ro => "Cum s-a schimbat compania pe luni?",
            Lang::Hu => "Hogyan változott a cég havonta?",
            Lang::Bg => "Как се променяше фирмата по месеци?",
            Lang::Be => "Як гэта кампанія змянялася па месяцах?",
            Lang::Zh => "这家公司按月如何变化？",
        },
        GuidedQuestionKind::CompanyGoodsByMonth => match lang {
            Lang::En => "Compare product codes by month",
            Lang::Ua => "Порівняти коди товару по місяцях",
            Lang::Ru => "Сравнить коды товара по месяцам",
            Lang::De => "Warencodes nach Monaten vergleichen",
            Lang::Es => "Comparar códigos de producto por mes",
            Lang::Fr => "Comparer les codes produit par mois",
            Lang::Pl => "Porównaj kody towarów według miesięcy",
            Lang::Pt => "Comparar códigos de produto por mês",
            Lang::Ro => "Compară codurile de produs pe luni",
            Lang::Hu => "Termékkódok összehasonlítása hónaponként",
            Lang::Bg => "Сравни кодовете по месеци",
            Lang::Be => "Параўнаць коды тавараў па месяцах",
            Lang::Zh => "按月比较商品编码",
        },
        GuidedQuestionKind::MarketCompanies => match lang {
            Lang::En => "Who are the biggest companies here?",
            Lang::Ua => "Хто найбільші компанії у вибірці?",
            Lang::Ru => "Кто крупнейшие компании в выборке?",
            Lang::De => "Wer sind hier die größten Firmen?",
            Lang::Es => "¿Cuáles son las empresas más grandes aquí?",
            Lang::Fr => "Quelles sont les plus grandes entreprises ici ?",
            Lang::Pl => "Które firmy są tu największe?",
            Lang::Pt => "Quais são as maiores empresas aqui?",
            Lang::Ro => "Care sunt cele mai mari companii aici?",
            Lang::Hu => "Melyek itt a legnagyobb cégek?",
            Lang::Bg => "Кои са най-големите фирми тук?",
            Lang::Be => "Хто найбуйнейшыя кампаніі ў выбарцы?",
            Lang::Zh => "这里最大的公司是谁？",
        },
        GuidedQuestionKind::MarketGoods => match lang {
            Lang::En => "Which goods dominate this slice?",
            Lang::Ua => "Які товари домінують у вибірці?",
            Lang::Ru => "Какие товары доминируют в выборке?",
            Lang::De => "Welche Waren dominieren diesen Ausschnitt?",
            Lang::Es => "¿Qué mercancías dominan esta selección?",
            Lang::Fr => "Quelles marchandises dominent ce périmètre ?",
            Lang::Pl => "Jakie towary dominują w tym zakresie?",
            Lang::Pt => "Quais mercadorias dominam este recorte?",
            Lang::Ro => "Ce mărfuri domină această selecție?",
            Lang::Hu => "Mely áruk dominálnak ebben a szűrésben?",
            Lang::Bg => "Кои стоки доминират в тази извадка?",
            Lang::Be => "Якія тавары дамінуюць у выбарцы?",
            Lang::Zh => "这个范围内哪些商品占主导？",
        },
        GuidedQuestionKind::MarketCountries => match lang {
            Lang::En => "Which countries and routes dominate?",
            Lang::Ua => "Які країни та маршрути домінують?",
            Lang::Ru => "Какие страны и маршруты доминируют?",
            Lang::De => "Welche Länder und Routen dominieren?",
            Lang::Es => "¿Qué países y rutas dominan?",
            Lang::Fr => "Quels pays et routes dominent ?",
            Lang::Pl => "Jakie kraje i trasy dominują?",
            Lang::Pt => "Quais países e rotas dominam?",
            Lang::Ro => "Ce țări și rute domină?",
            Lang::Hu => "Mely országok és útvonalak dominálnak?",
            Lang::Bg => "Кои държави и маршрути доминират?",
            Lang::Be => "Якія краіны і маршруты дамінуюць?",
            Lang::Zh => "哪些国家和路线占主导？",
        },
        GuidedQuestionKind::MarketPrices => match lang {
            Lang::En => "Are prices normal in this slice?",
            Lang::Ua => "Чи нормальні ціни в цій вибірці?",
            Lang::Ru => "Нормальные ли цены в этой выборке?",
            Lang::De => "Sind die Preise in diesem Ausschnitt normal?",
            Lang::Es => "¿Son normales los precios en esta selección?",
            Lang::Fr => "Les prix sont-ils normaux dans ce périmètre ?",
            Lang::Pl => "Czy ceny w tym zakresie są normalne?",
            Lang::Pt => "Os preços são normais neste recorte?",
            Lang::Ro => "Sunt prețurile normale în această selecție?",
            Lang::Hu => "Normálisak az árak ebben a szűrésben?",
            Lang::Bg => "Нормални ли са цените в тази извадка?",
            Lang::Be => "Ці нармальныя цэны ў гэтай выбарцы?",
            Lang::Zh => "这个范围内价格是否正常？",
        },
    }
}

fn exact_edrpou_candidate(text: &str, filters: &Filters) -> Option<String> {
    let from_filter = filters.edrpou.trim();
    if !from_filter.is_empty() {
        return Some(from_filter.to_string());
    }
    let text = text.trim();
    if text.len() == 8 && text.chars().all(|c| c.is_ascii_digit()) {
        Some(text.to_string())
    } else {
        None
    }
}

fn guided_questions_for(
    text: &str,
    filters: &Filters,
) -> Vec<(GuidedQuestionSection, GuidedQuestionKind)> {
    let mut out = Vec::new();
    let has_text = !text.trim().is_empty();
    let has_product = has_text
        || !filters.product_code.trim().is_empty()
        || !filters.trademark.trim().is_empty()
        || !filters.description.trim().is_empty();
    let has_company = exact_edrpou_candidate(text, filters).is_some()
        || !filters.recipient.trim().is_empty()
        || !filters.sender.trim().is_empty();
    let has_market = !filters.year.trim().is_empty()
        || !filters.origin_country.trim().is_empty()
        || !filters.dispatch_country.trim().is_empty()
        || !filters.trade_country.trim().is_empty();

    if has_product {
        out.extend([
            (
                GuidedQuestionSection::Product,
                GuidedQuestionKind::ProductCompanies,
            ),
            (
                GuidedQuestionSection::Product,
                GuidedQuestionKind::ProductAllCompanies,
            ),
            (
                GuidedQuestionSection::Product,
                GuidedQuestionKind::ProductGoods,
            ),
            (
                GuidedQuestionSection::Product,
                GuidedQuestionKind::ProductCountries,
            ),
            (
                GuidedQuestionSection::Product,
                GuidedQuestionKind::ProductPrices,
            ),
            (
                GuidedQuestionSection::Product,
                GuidedQuestionKind::ProductTimeline,
            ),
            (
                GuidedQuestionSection::Product,
                GuidedQuestionKind::ProductCompaniesByMonth,
            ),
        ]);
    }
    if has_company {
        if exact_edrpou_candidate(text, filters).is_some() {
            out.push((
                GuidedQuestionSection::Company,
                GuidedQuestionKind::CompanyProfile,
            ));
        }
        out.extend([
            (
                GuidedQuestionSection::Company,
                GuidedQuestionKind::CompanyGoods,
            ),
            (
                GuidedQuestionSection::Company,
                GuidedQuestionKind::CompanySuppliers,
            ),
            (
                GuidedQuestionSection::Company,
                GuidedQuestionKind::CompanyCountries,
            ),
            (
                GuidedQuestionSection::Company,
                GuidedQuestionKind::CompanyTimeline,
            ),
            (
                GuidedQuestionSection::Company,
                GuidedQuestionKind::CompanyGoodsByMonth,
            ),
        ]);
    }
    if has_market || (!has_product && !has_company) {
        out.extend([
            (
                GuidedQuestionSection::Market,
                GuidedQuestionKind::MarketCompanies,
            ),
            (
                GuidedQuestionSection::Market,
                GuidedQuestionKind::MarketGoods,
            ),
            (
                GuidedQuestionSection::Market,
                GuidedQuestionKind::MarketCountries,
            ),
            (
                GuidedQuestionSection::Market,
                GuidedQuestionKind::MarketPrices,
            ),
        ]);
    }
    out
}

fn guided_question_action(
    kind: GuidedQuestionKind,
    text: &str,
    filters: &Filters,
) -> Option<GuidedQuestionAction> {
    Some(match kind {
        GuidedQuestionKind::ProductCompanies | GuidedQuestionKind::MarketCompanies => {
            GuidedQuestionAction::Analytics(AnalyticsView::Companies)
        }
        GuidedQuestionKind::ProductAllCompanies => {
            GuidedQuestionAction::Explore(AnalyticsSectionKind::Edrpou)
        }
        GuidedQuestionKind::ProductGoods
        | GuidedQuestionKind::CompanyGoods
        | GuidedQuestionKind::MarketGoods => {
            GuidedQuestionAction::Analytics(AnalyticsView::Products)
        }
        GuidedQuestionKind::ProductCountries
        | GuidedQuestionKind::CompanyCountries
        | GuidedQuestionKind::MarketCountries => {
            GuidedQuestionAction::Analytics(AnalyticsView::Countries)
        }
        GuidedQuestionKind::ProductPrices | GuidedQuestionKind::MarketPrices => {
            GuidedQuestionAction::Analytics(AnalyticsView::Prices)
        }
        GuidedQuestionKind::ProductTimeline | GuidedQuestionKind::CompanyTimeline => {
            GuidedQuestionAction::Analytics(AnalyticsView::Overview)
        }
        GuidedQuestionKind::ProductCompaniesByMonth => {
            GuidedQuestionAction::Pivot(PivotDim::Recipient, PivotDim::Month, PivotMetric::Value)
        }
        GuidedQuestionKind::CompanyProfile => {
            GuidedQuestionAction::Profile(exact_edrpou_candidate(text, filters)?)
        }
        GuidedQuestionKind::CompanySuppliers => {
            GuidedQuestionAction::Explore(AnalyticsSectionKind::Senders)
        }
        GuidedQuestionKind::CompanyGoodsByMonth => {
            GuidedQuestionAction::Pivot(PivotDim::ProductCode, PivotDim::Month, PivotMetric::Value)
        }
    })
}

fn analytics_calc_title(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Як рахуються цифри",
        Lang::Ru => "Как считаются цифры",
        _ => "How the numbers are calculated",
    }
}

fn analytics_calc_short_note(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Усі цифри рахуються за поточним запитом і фільтрами.",
        Lang::Ru => "Все цифры считаются по текущему запросу и фильтрам.",
        _ => "All numbers are calculated from the current search and filters.",
    }
}

fn analytics_calc_lines(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Ua => &[
            "Рядки = знайдені товарні рядки, не унікальні декларації.",
            "Декларації = унікальні номери МД у поточній вибірці.",
            "Сума = сума поля «ФВ вал.контр», якщо воно заповнене у джерелі.",
            "$/кг = сума / нетто; якщо нетто порожнє або нульове, показник не рахується.",
            "У групах частка рахується від суми; якщо суми немає, використовується нетто, потім кількість рядків.",
            "Аналітика рахує унікальні рядки: дублікати, позначені як повтори, не подвоюють підсумки.",
        ],
        Lang::Ru => &[
            "Строки = найденные товарные строки, не уникальные декларации.",
            "Декларации = уникальные номера МД в текущей выборке.",
            "Сумма = сумма поля «ФВ вал.контр», если оно заполнено в источнике.",
            "$/кг = сумма / нетто; если нетто пустое или нулевое, показатель не считается.",
            "В группах доля считается от суммы; если суммы нет, используется нетто, затем количество строк.",
            "Аналитика считает уникальные строки: дубликаты, помеченные как повторы, не удваивают итоги.",
        ],
        _ => &[
            "Rows = matching product rows, not unique declarations.",
            "Declarations = distinct declaration numbers in the current result set.",
            "Value = SUM of the source field “ФВ вал.контр” when it is filled.",
            "$/kg = value / net kg; empty or zero net weight is skipped.",
            "Group share uses value first; if value is empty, it falls back to net weight, then row count.",
            "Analytics counts unique rows: duplicate rows flagged as repeats do not double totals.",
        ],
    }
}

fn price_average_help(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Звичайне середнє за рядками з числовим значенням.",
        Lang::Ru => "Обычное среднее по строкам с числовым значением.",
        _ => "Simple average across rows with a numeric value.",
    }
}

fn price_weighted_help(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Середнє, зважене за нетто кг: SUM(ціна * нетто) / SUM(нетто).",
        Lang::Ru => "Среднее, взвешенное по нетто кг: SUM(цена * нетто) / SUM(нетто).",
        _ => "Net-kg weighted average: SUM(price * net kg) / SUM(net kg).",
    }
}

fn price_median_help(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Медіана: половина значень нижче, половина вище.",
        Lang::Ru => "Медиана: половина значений ниже, половина выше.",
        _ => "Median: half the values are lower and half are higher.",
    }
}

fn price_range_help(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "P25-P75: середній діапазон без крайніх 25% знизу і зверху.",
        Lang::Ru => "P25-P75: средний диапазон без крайних 25% снизу и сверху.",
        _ => "P25-P75: middle range after excluding the lowest and highest quarters.",
    }
}

fn price_count_help(lang: Lang) -> &'static str {
    match lang {
        Lang::Ua => "Кількість рядків, де цей показник можна прочитати як число.",
        Lang::Ru => "Количество строк, где этот показатель можно прочитать как число.",
        _ => "Rows where this metric can be parsed as a number.",
    }
}

/// Database location: a `data` folder beside the executable (a portable
/// install) or, when that location is not writable (e.g. /usr/bin on Linux
/// or /Applications on macOS), a folder in the user's home directory.
pub fn default_db_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let portable = exe_dir.join("data");
    if std::fs::create_dir_all(&portable).is_ok() {
        return portable.join("base_search.db");
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".base-search").join("base_search.db")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Import,
    Export,
    Clear,
}

struct OpState {
    kind: OpKind,
    cancel: Arc<AtomicBool>,
    last_event: Option<ImportEvent>,
    export_progress: (u64, u64),
}

#[derive(Default)]
struct StatusLine {
    text: String,
    is_error: bool,
}

#[derive(Clone)]
struct StoredQuery {
    name: String,
    query: Query,
}

fn invalidate_underpricing_generation(generation: &mut u64) {
    *generation = generation.wrapping_add(1);
}

pub struct App {
    lang: Lang,
    db_path: PathBuf,
    /// Lightweight connection for instant operations, such as cards and settings.
    lite_db: Option<Db>,

    query_text: String,
    filters: Filters,
    recent_queries: Vec<StoredQuery>,
    saved_queries: Vec<StoredQuery>,
    show_filters: bool,
    active_query: Query,
    page: u64,
    total: Option<u64>,
    rows: Vec<Vec<String>>,
    row_ids: Vec<i64>,
    page_has_next: bool,
    /// Per result row: Some(first file) when the row is a kept duplicate.
    result_dups: Vec<Option<String>>,
    analytics: Option<Analytics>,
    active_tab: AppTab,
    analytics_limit: u64,
    /// Generation of the query the loaded analytics belong to.
    analytics_gen: u64,
    /// Active sub-tab on the Analytics view.
    analytics_view: AnalyticsView,
    /// Which sub-tabs are loaded for `analytics_gen` (indexed by view).
    analytics_loaded: [bool; 6],
    analytics_loading: bool,
    /// Product code grouping level: 2/4/6 digits or 10 for full codes.
    hs_level: u8,
    group_explorer: Option<GroupExplorerState>,
    month_metric: MonthMetric,
    /// Pivot (cross-tab) state.
    pivot: Option<PivotResult>,
    pivot_row_dim: PivotDim,
    pivot_col_dim: PivotDim,
    pivot_metric: PivotMetric,
    /// Undervaluation scan (in the Prices sub-tab).
    underpricing: Option<Undervaluation>,
    underpricing_loading: bool,
    underpricing_gen: u64,
    selected: HashSet<usize>,
    select_anchor: Option<usize>,
    visible_cols: Vec<bool>,
    search_gen: u64,
    search_in_flight: bool,
    count_in_flight: bool,
    last_search_ms: Option<u64>,

    db_total_rows: Option<u64>,
    status: StatusLine,

    op: Option<OpState>,
    import_report: Option<Vec<FileSummary>>,

    card: Option<RecordCard>,
    card_open: bool,
    show_settings: bool,
    show_help: bool,
    confirm_clear: bool,

    /// Open company dossier; `None` means the normal Results/Analytics view.
    profile: Option<CompanyProfile>,
    profile_loading: bool,
    profile_gen: u64,

    msg_rx: Receiver<Msg>,
    msg_tx: Sender<Msg>,
    search_tx: Sender<WorkerReq>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);

        let db_path = default_db_path();
        let lite_db = Db::open(&db_path).ok();

        let lang = lite_db
            .as_ref()
            .and_then(|db| db.meta_get("lang"))
            .map(|c| Lang::from_code(&c))
            .unwrap_or_default();
        // Show the quick guide automatically on the very first launch.
        let first_run = lite_db
            .as_ref()
            .map(|db| db.meta_get("help_seen").is_none())
            .unwrap_or(false);
        let theme = lite_db.as_ref().and_then(|db| db.meta_get("theme"));
        cc.egui_ctx.set_theme(match theme.as_deref() {
            Some("dark") => egui::Theme::Dark,
            _ => egui::Theme::Light,
        });
        if let Some(zoom) = lite_db
            .as_ref()
            .and_then(|db| db.meta_get("zoom"))
            .and_then(|z| z.parse::<f32>().ok())
        {
            cc.egui_ctx.set_zoom_factor(zoom.clamp(0.6, 2.0));
        }
        let hidden: HashSet<String> = lite_db
            .as_ref()
            .and_then(|db| db.meta_get("hidden_cols"))
            .map(|s| s.split(',').map(str::to_owned).collect())
            .unwrap_or_default();
        let visible_cols = RESULT_COLUMNS
            .iter()
            .map(|name| !hidden.contains(*name))
            .collect();
        let recent_queries = lite_db
            .as_ref()
            .and_then(|db| db.meta_get(RECENT_QUERIES_META))
            .map(|raw| decode_stored_queries(&raw))
            .unwrap_or_default();
        let saved_queries = lite_db
            .as_ref()
            .and_then(|db| db.meta_get(SAVED_QUERIES_META))
            .map(|raw| decode_stored_queries(&raw))
            .unwrap_or_default();

        let (msg_tx, msg_rx) = channel::<Msg>();
        let (search_tx, search_rx) = channel::<WorkerReq>();
        workers::spawn_search_worker(
            db_path.clone(),
            search_rx,
            msg_tx.clone(),
            cc.egui_ctx.clone(),
        );

        let mut app = App {
            lang,
            db_path,
            lite_db,
            query_text: String::new(),
            filters: Filters::default(),
            recent_queries,
            saved_queries,
            show_filters: false,
            active_query: Query::default(),
            page: 0,
            total: None,
            rows: Vec::new(),
            row_ids: Vec::new(),
            page_has_next: false,
            result_dups: Vec::new(),
            analytics: None,
            active_tab: AppTab::Results,
            analytics_limit: 10,
            analytics_gen: 0,
            analytics_view: AnalyticsView::default(),
            analytics_loaded: [false; 6],
            analytics_loading: false,
            hs_level: 10,
            group_explorer: None,
            month_metric: MonthMetric::default(),
            pivot: None,
            pivot_row_dim: PivotDim::Recipient,
            pivot_col_dim: PivotDim::Month,
            pivot_metric: PivotMetric::Value,
            underpricing: None,
            underpricing_loading: false,
            underpricing_gen: 0,
            selected: HashSet::new(),
            select_anchor: None,
            visible_cols,
            search_gen: 0,
            search_in_flight: false,
            count_in_flight: false,
            last_search_ms: None,
            db_total_rows: None,
            status: StatusLine::default(),
            op: None,
            import_report: None,
            card: None,
            card_open: false,
            show_settings: false,
            show_help: first_run,
            confirm_clear: false,
            profile: None,
            profile_loading: false,
            profile_gen: 0,
            msg_rx,
            msg_tx,
            search_tx,
        };
        let _ = app.search_tx.send(WorkerReq::Stats);
        app.start_search(true);

        // Repair the search index if the previous run was interrupted.
        if let Some(db) = &app.lite_db
            && db.unindexed_rows() > 0
        {
            let cancel = Arc::new(AtomicBool::new(false));
            app.op = Some(OpState {
                kind: OpKind::Import,
                cancel: cancel.clone(),
                last_event: None,
                export_progress: (0, 0),
            });
            workers::spawn_index_repair(
                app.db_path.clone(),
                cancel,
                app.msg_tx.clone(),
                cc.egui_ctx.clone(),
            );
        }
        app
    }

    fn t(&self) -> &'static Tr {
        tr(self.lang)
    }

    fn persist(&self, key: &str, value: &str) {
        if let Some(db) = &self.lite_db {
            db.meta_set(key, value);
        }
    }

    fn persist_hidden_cols(&self) {
        let hidden: Vec<&str> = RESULT_COLUMNS
            .iter()
            .zip(&self.visible_cols)
            .filter(|(_, v)| !**v)
            .map(|(n, _)| *n)
            .collect();
        self.persist("hidden_cols", &hidden.join(","));
    }

    fn persist_recent_queries(&self) {
        self.persist(
            RECENT_QUERIES_META,
            &encode_stored_queries(&self.recent_queries),
        );
    }

    fn persist_saved_queries(&self) {
        self.persist(
            SAVED_QUERIES_META,
            &encode_stored_queries(&self.saved_queries),
        );
    }

    fn remember_recent_query(&mut self, query: &Query) {
        if query.is_empty() {
            return;
        }
        self.recent_queries.retain(|item| item.query != *query);
        self.recent_queries.insert(
            0,
            StoredQuery {
                name: query_summary(query, self.t()),
                query: query.clone(),
            },
        );
        self.recent_queries.truncate(RECENT_QUERY_LIMIT);
        self.persist_recent_queries();
    }

    fn save_current_query(&mut self) {
        let query = Query {
            text: self.query_text.clone(),
            filters: self.filters.clone(),
        };
        if query.is_empty() {
            return;
        }
        self.saved_queries.retain(|item| item.query != query);
        self.saved_queries.insert(
            0,
            StoredQuery {
                name: query_summary(&query, self.t()),
                query,
            },
        );
        self.persist_saved_queries();
    }

    fn clear_recent_queries(&mut self) {
        self.recent_queries.clear();
        self.persist_recent_queries();
    }

    fn remove_saved_query(&mut self, index: usize) {
        if index < self.saved_queries.len() {
            self.saved_queries.remove(index);
            self.persist_saved_queries();
        }
    }

    fn apply_stored_query(&mut self, query: Query) {
        self.query_text = query.text;
        self.filters = query.filters;
        self.show_filters = !self.filters.is_empty();
        self.active_tab = AppTab::Results;
        self.start_search(true);
    }

    fn start_search(&mut self, reset_page: bool) {
        if reset_page {
            self.page = 0;
        }
        self.active_query = Query {
            text: self.query_text.clone(),
            filters: self.filters.clone(),
        };
        let query_to_remember = self.active_query.clone();
        if reset_page {
            self.remember_recent_query(&query_to_remember);
        }
        self.search_gen += 1;
        self.search_in_flight = true;
        self.count_in_flight = true;
        self.page_has_next = false;
        self.total = None;
        self.last_search_ms = None;
        // The query changed; loaded analytics no longer matches the results.
        self.analytics = None;
        self.analytics_loaded = [false; 6];
        self.analytics_loading = false;
        self.group_explorer = None;
        self.pivot = None;
        self.underpricing = None;
        self.underpricing_loading = false;
        invalidate_underpricing_generation(&mut self.underpricing_gen);
        let _ = self.search_tx.send(WorkerReq::Search {
            q: Box::new(self.active_query.clone()),
            page: self.page,
            generation: self.search_gen,
        });
        if self.active_tab == AppTab::Analytics {
            self.request_analytics();
        }
    }

    fn goto_page(&mut self, page: u64) {
        self.page = page;
        self.search_gen += 1;
        self.search_in_flight = true;
        self.count_in_flight = true;
        self.page_has_next = false;
        self.total = None;
        self.last_search_ms = None;
        let _ = self.search_tx.send(WorkerReq::Search {
            q: Box::new(self.active_query.clone()),
            page,
            generation: self.search_gen,
        });
    }

    /// Requests the active Analytics sub-tab if it has not been loaded yet.
    fn request_analytics(&mut self) {
        if self.active_query.is_empty() {
            return;
        }
        if self.analytics_view == AnalyticsView::Pivot {
            // Pivot needs the headline overview too (summary line + guard);
            // load it once if it is missing for this query.
            if self.analytics.is_none() || self.analytics_gen != self.search_gen {
                self.analytics_loading = true;
                let _ = self.search_tx.send(WorkerReq::Analytics {
                    q: Box::new(self.active_query.clone()),
                    limit: self.analytics_limit,
                    scope: None,
                    hs_level: self.hs_level,
                    generation: self.search_gen,
                });
            }
            self.request_pivot();
            return;
        }
        if self.analytics_gen == self.search_gen
            && self.analytics_loaded[self.analytics_view.index()]
        {
            return;
        }
        self.analytics_loading = true;
        let _ = self.search_tx.send(WorkerReq::Analytics {
            q: Box::new(self.active_query.clone()),
            limit: self.analytics_limit,
            scope: self.analytics_view.scope(),
            hs_level: self.hs_level,
            generation: self.search_gen,
        });
    }

    fn open_group_explorer(&mut self, kind: AnalyticsSectionKind) {
        if self.active_query.is_empty() {
            return;
        }
        self.group_explorer = Some(GroupExplorerState {
            kind,
            generation: self.search_gen,
            loading: true,
            rows: Vec::new(),
            label_filter: String::new(),
            sort: GroupSort::Value,
            descending: true,
        });
        let _ = self.search_tx.send(WorkerReq::AnalyticsSection {
            q: Box::new(self.active_query.clone()),
            kind,
            limit: FULL_SECTION_LIMIT,
            hs_level: self.hs_level,
            generation: self.search_gen,
        });
    }

    /// Scans the current query for declarations priced far below the median
    /// for their product code.
    fn request_underpricing(&mut self) {
        if self.active_query.is_empty() {
            return;
        }
        self.underpricing = None;
        self.underpricing_loading = true;
        invalidate_underpricing_generation(&mut self.underpricing_gen);
        let _ = self.search_tx.send(WorkerReq::Underpricing {
            q: Box::new(self.active_query.clone()),
            threshold: 0.5,
            generation: self.underpricing_gen,
        });
    }

    /// (Re)builds the pivot for the current query and chosen dimensions.
    fn request_pivot(&mut self) {
        if self.active_query.is_empty() {
            return;
        }
        self.pivot = None;
        self.analytics_loaded[AnalyticsView::Pivot.index()] = false;
        self.analytics_loading = true;
        let others = self.t().others;
        let _ = self.search_tx.send(WorkerReq::Pivot {
            q: Box::new(self.active_query.clone()),
            row_dim: self.pivot_row_dim,
            col_dim: self.pivot_col_dim,
            metric: self.pivot_metric,
            others_label: others.to_string(),
            generation: self.search_gen,
        });
    }

    fn page_count(&self) -> u64 {
        match self.total {
            Some(total) => total.div_ceil(PAGE_SIZE).max(1),
            None if !self.search_in_flight && self.page_has_next => self.page + 2,
            None => self.page + 1,
        }
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                Msg::SearchPage {
                    generation,
                    ids,
                    rows,
                    dups,
                    has_next,
                    ms,
                } => {
                    if generation == self.search_gen {
                        self.row_ids = ids;
                        self.rows = rows;
                        self.result_dups = dups;
                        self.page_has_next = has_next;
                        if self.page == 0 && self.rows.is_empty() {
                            self.total = Some(0);
                            self.count_in_flight = false;
                        }
                        self.last_search_ms = Some(ms);
                        self.search_in_flight = false;
                        self.selected.clear();
                        self.select_anchor = None;
                    }
                }
                Msg::SearchCount { generation, total } => {
                    if generation == self.search_gen {
                        self.total = Some(total);
                        self.count_in_flight = false;
                    }
                }
                Msg::AnalyticsDone {
                    generation,
                    scope,
                    analytics,
                } => {
                    if generation == self.search_gen {
                        match self.analytics.as_mut() {
                            Some(existing) if self.analytics_gen == generation => {
                                // Load one section at a time: overview and
                                // months stay fresh, sections are merged into
                                // the shared analytics container.
                                existing.overview = analytics.overview;
                                existing.months = analytics.months;
                                match scope {
                                    None => {}
                                    Some(AnalyticsScope::Companies) => {
                                        existing.company_sections = analytics.company_sections;
                                    }
                                    Some(AnalyticsScope::Products) => {
                                        existing.product_sections = analytics.product_sections;
                                    }
                                    Some(AnalyticsScope::Countries) => {
                                        existing.country_sections = analytics.country_sections;
                                    }
                                    Some(AnalyticsScope::Prices) => {
                                        existing.price_sections = analytics.price_sections;
                                    }
                                }
                            }
                            _ => {
                                self.analytics = Some(*analytics);
                                self.analytics_loaded = [false; 6];
                            }
                        }
                        self.analytics_gen = generation;
                        self.analytics_loaded[AnalyticsView::from_scope(scope).index()] = true;
                        self.analytics_loading = false;
                    }
                }
                Msg::AnalyticsSectionDone {
                    generation,
                    section,
                } => {
                    if let Some(explorer) = &mut self.group_explorer
                        && explorer.generation == generation
                        && explorer.kind == section.kind
                    {
                        explorer.rows = section.rows;
                        explorer.loading = false;
                    }
                }
                Msg::SearchError {
                    generation,
                    message,
                } => {
                    if generation == self.search_gen {
                        self.search_in_flight = false;
                        self.count_in_flight = false;
                        self.analytics_loading = false;
                        self.status = StatusLine {
                            text: format!("{}: {message}", self.t().error),
                            is_error: true,
                        };
                    }
                    if let Some(explorer) = &mut self.group_explorer
                        && explorer.generation == generation
                    {
                        explorer.loading = false;
                    }
                }
                Msg::ProfileDone {
                    generation,
                    profile,
                } => {
                    if generation == self.profile_gen {
                        self.profile = Some(*profile);
                        self.profile_loading = false;
                    }
                }
                Msg::PivotDone { generation, pivot } => {
                    if generation == self.search_gen {
                        self.pivot = Some(*pivot);
                        self.analytics_gen = generation;
                        self.analytics_loaded[AnalyticsView::Pivot.index()] = true;
                        self.analytics_loading = false;
                    }
                }
                Msg::UnderpricingDone { generation, result } => {
                    if generation == self.underpricing_gen {
                        self.underpricing = Some(*result);
                        self.underpricing_loading = false;
                    }
                }
                Msg::Stats(total) => self.db_total_rows = Some(total),
                Msg::Import(ev) => {
                    if let Some(op) = &mut self.op {
                        op.last_event = Some(ev);
                    }
                }
                Msg::ImportDone(summaries, total_rows) => {
                    self.op = None;
                    self.db_total_rows = Some(total_rows);
                    if !summaries.is_empty() {
                        let imported: u64 = summaries.iter().map(|s| s.imported).sum();
                        let dups: u64 = summaries.iter().map(|s| s.duplicates).sum();
                        let errors = summaries.iter().filter(|s| s.error.is_some()).count();
                        self.status = StatusLine {
                            text: fmt(
                                self.t().import_done,
                                &[
                                    &group_digits(imported),
                                    &group_digits(dups),
                                    &errors.to_string(),
                                ],
                            ),
                            is_error: errors > 0,
                        };
                        self.import_report = Some(summaries);
                    }
                    let _ = self.search_tx.send(WorkerReq::Stats);
                    self.start_search(true);
                }
                Msg::ExportProgress(done, total) => {
                    if let Some(op) = &mut self.op {
                        op.export_progress = (done, total);
                    }
                }
                Msg::ExportDone(result) => {
                    self.op = None;
                    self.status = match result {
                        Ok((written, path)) => StatusLine {
                            text: format!(
                                "{} \u{2192} {}",
                                fmt(self.t().export_done, &[&group_digits(written)]),
                                path.display()
                            ),
                            is_error: false,
                        },
                        Err(ExportError::TooManyRowsForXlsx(_)) => StatusLine {
                            text: self.t().xlsx_limit.to_string(),
                            is_error: true,
                        },
                        Err(ExportError::Cancelled) => StatusLine {
                            text: self.t().cancelled.to_string(),
                            is_error: false,
                        },
                        Err(ExportError::Other(e)) => StatusLine {
                            text: format!("{}: {e}", self.t().error),
                            is_error: true,
                        },
                    };
                }
                Msg::DbCleared(result) => {
                    self.op = None;
                    self.status = match result {
                        Ok(()) => StatusLine {
                            text: self.t().db_cleared.to_string(),
                            is_error: false,
                        },
                        Err(e) => StatusLine {
                            text: format!("{}: {e}", self.t().error),
                            is_error: true,
                        },
                    };
                    let _ = self.search_tx.send(WorkerReq::Stats);
                    self.start_search(true);
                }
                Msg::Fatal(message) => {
                    self.status = StatusLine {
                        text: format!("{}: {message}", self.t().error),
                        is_error: true,
                    };
                }
            }
        }
    }

    fn pick_and_import(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let files = rfd::FileDialog::new()
            .set_title(t.choose_files)
            .add_filter(t.excel_files, &["xlsx", "xlsb", "xls"])
            .pick_files();
        let Some(files) = files else { return };
        if files.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.op = Some(OpState {
            kind: OpKind::Import,
            cancel: cancel.clone(),
            last_event: None,
            export_progress: (0, 0),
        });
        self.status = StatusLine::default();
        workers::spawn_import(
            self.db_path.clone(),
            files,
            cancel,
            self.msg_tx.clone(),
            ctx.clone(),
        );
    }

    fn pick_and_export(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let dest = rfd::FileDialog::new()
            .set_title(t.save_as)
            .add_filter("CSV", &["csv"])
            .add_filter("Excel", &["xlsx"])
            .set_file_name("base_search_export.csv")
            .save_file();
        let Some(mut dest) = dest else { return };
        if dest.extension().is_none() {
            dest.set_extension("csv");
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.op = Some(OpState {
            kind: OpKind::Export,
            cancel: cancel.clone(),
            last_event: None,
            export_progress: (0, 0),
        });
        self.status = StatusLine::default();
        workers::spawn_export(
            self.db_path.clone(),
            self.active_query.clone(),
            dest,
            cancel,
            self.msg_tx.clone(),
            ctx.clone(),
        );
    }

    fn start_clear_db(&mut self, ctx: &egui::Context) {
        self.op = Some(OpState {
            kind: OpKind::Clear,
            cancel: Arc::new(AtomicBool::new(false)),
            last_event: None,
            export_progress: (0, 0),
        });
        self.status = StatusLine::default();
        workers::spawn_clear_db(self.db_path.clone(), self.msg_tx.clone(), ctx.clone());
    }

    fn open_card(&mut self, row_index: usize) {
        let Some(id) = self.row_ids.get(row_index).copied() else {
            return;
        };
        if let Some(db) = &self.lite_db
            && let Ok(card) = db.record_card(id)
        {
            self.card = Some(card);
            self.card_open = true;
        }
    }

    fn open_card_by_id(&mut self, id: i64) {
        if let Some(db) = &self.lite_db
            && let Ok(card) = db.record_card(id)
        {
            self.card = Some(card);
            self.card_open = true;
        }
    }

    /// Opens (or refreshes) the company dossier for an EDRPOU in the background.
    fn open_profile(&mut self, edrpou: String) {
        let edrpou = edrpou.trim().to_string();
        if edrpou.is_empty() {
            return;
        }
        self.profile = None;
        self.profile_loading = true;
        self.profile_gen += 1;
        let _ = self.search_tx.send(WorkerReq::Profile {
            edrpou,
            generation: self.profile_gen,
        });
    }

    fn close_profile(&mut self) {
        self.profile = None;
        self.profile_loading = false;
        self.profile_gen += 1;
    }

    fn run_guided_question(&mut self, action: GuidedQuestionAction) {
        let current = Query {
            text: self.query_text.clone(),
            filters: self.filters.clone(),
        };
        if current.is_empty() && !matches!(action, GuidedQuestionAction::Profile(_)) {
            self.status = StatusLine {
                text: guided_questions_empty(self.lang).to_string(),
                is_error: false,
            };
            return;
        }
        let query_changed = current != self.active_query;
        match action {
            GuidedQuestionAction::Analytics(view) => {
                self.active_tab = AppTab::Analytics;
                self.analytics_view = view;
                if query_changed {
                    self.start_search(true);
                } else {
                    self.request_analytics();
                }
            }
            GuidedQuestionAction::Explore(kind) => {
                self.active_tab = AppTab::Analytics;
                if query_changed {
                    self.start_search(true);
                }
                self.open_group_explorer(kind);
            }
            GuidedQuestionAction::Pivot(row_dim, col_dim, metric) => {
                self.active_tab = AppTab::Analytics;
                self.analytics_view = AnalyticsView::Pivot;
                self.pivot_row_dim = row_dim;
                self.pivot_col_dim = col_dim;
                self.pivot_metric = metric;
                self.pivot = None;
                self.analytics_loaded[AnalyticsView::Pivot.index()] = false;
                if query_changed {
                    self.start_search(true);
                } else {
                    self.request_analytics();
                }
            }
            GuidedQuestionAction::Profile(edrpou) => self.open_profile(edrpou),
        }
    }

    fn handle_row_click(&mut self, i: usize, modifiers: egui::Modifiers) {
        if modifiers.ctrl || modifiers.command {
            if !self.selected.insert(i) {
                self.selected.remove(&i);
            }
            self.select_anchor = Some(i);
        } else if modifiers.shift && self.select_anchor.is_some() {
            let anchor = self.select_anchor.unwrap();
            let (lo, hi) = (anchor.min(i), anchor.max(i));
            self.selected = (lo..=hi).collect();
        } else {
            self.selected.clear();
            self.selected.insert(i);
            self.select_anchor = Some(i);
        }
    }

    /// Copies selected rows as TSV using visible columns, ready to paste into Excel.
    fn copy_selected_rows(&self, ctx: &egui::Context) {
        let mut indices: Vec<usize> = self.selected.iter().copied().collect();
        indices.sort_unstable();
        let lines: Vec<String> = indices
            .iter()
            .filter_map(|i| self.rows.get(*i))
            .map(|row| {
                row.iter()
                    .zip(&self.visible_cols)
                    .filter(|(_, v)| **v)
                    .map(|(value, _)| value.as_str())
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect();
        if !lines.is_empty() {
            ctx.copy_text(lines.join("\n"));
        }
    }

    fn apply_menu_action(&mut self, ctx: &egui::Context, action: RowMenuAction) {
        let quick_filter = |this: &mut Self, set: &dyn Fn(&mut Filters, String), value: String| {
            this.query_text.clear();
            this.filters.clear();
            set(&mut this.filters, value);
            this.show_filters = true;
            this.start_search(true);
        };
        match action {
            RowMenuAction::CopyCell(value) => ctx.copy_text(value),
            RowMenuAction::CopyRow(i) => {
                if let Some(row) = self.rows.get(i) {
                    ctx.copy_text(row.join("\t"));
                }
            }
            RowMenuAction::CopySelected => self.copy_selected_rows(ctx),
            RowMenuAction::FilterSender(v) => {
                quick_filter(self, &|f, v| f.sender = v, v);
            }
            RowMenuAction::FilterRecipient(v) => {
                quick_filter(self, &|f, v| f.recipient = v, v);
            }
            RowMenuAction::FilterCode(v) => {
                quick_filter(self, &|f, v| f.product_code = v, v);
            }
            RowMenuAction::FilterEdrpou(v) => {
                quick_filter(self, &|f, v| f.edrpou = v, v);
            }
            RowMenuAction::OpenProfile(v) => self.open_profile(v),
        }
    }

    fn apply_analytics_filter(&mut self, action: AnalyticsFilterAction) {
        match action.field {
            AnalyticsFilterField::Recipient => self.filters.recipient = action.value,
            AnalyticsFilterField::Sender => self.filters.sender = action.value,
            AnalyticsFilterField::Edrpou => self.filters.edrpou = action.value,
            AnalyticsFilterField::ProductCode => self.filters.product_code = action.value,
            AnalyticsFilterField::OriginCountry => self.filters.origin_country = action.value,
            AnalyticsFilterField::DispatchCountry => self.filters.dispatch_country = action.value,
            AnalyticsFilterField::TradeCountry => self.filters.trade_country = action.value,
            AnalyticsFilterField::Trademark => self.filters.trademark = action.value,
            AnalyticsFilterField::Description => self.filters.description = action.value,
        }
        self.show_filters = true;
        self.active_tab = AppTab::Results;
        self.start_search(true);
    }

    // ---------- panels ----------

    fn ui_toolbar(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        let t = self.t();
        let mut do_search = false;
        let mut do_import = false;
        let mut do_export = false;
        let mut switched_to_analytics = false;
        let mut apply_stored_query: Option<Query> = None;
        let mut clear_recent_queries = false;
        let mut save_current_query = false;
        let mut remove_saved_query: Option<usize> = None;
        let mut guided_action: Option<GuidedQuestionAction> = None;
        let recent_queries = self.recent_queries.clone();
        let saved_queries = self.saved_queries.clone();
        let guided_filters = self.filters.clone();
        let guided_text = self.query_text.clone();
        let guided_query = Query {
            text: guided_text.clone(),
            filters: guided_filters.clone(),
        };
        let guided_items = guided_questions_for(&guided_text, &guided_filters);
        let frame = egui::Frame::side_top_panel(&ctx.global_style()).inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 10,
            bottom: 8,
        });
        egui::Panel::top("toolbar")
            .frame(frame)
            .show_inside(root, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(t.app_title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("\u{2699}").on_hover_text(t.settings).clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        if ui
                            .button("?")
                            .on_hover_text(format!("{} (F1)", t.help))
                            .clicked()
                        {
                            self.show_help = true;
                        }
                        ui.separator();
                        if let Some(total) = self.db_total_rows {
                            ui.label(
                                egui::RichText::new(fmt(t.db_rows, &[&group_digits(total)])).weak(),
                            );
                        }
                    });
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    let busy = self.op.is_some();
                    if ui.add_enabled(!busy, egui::Button::new(t.import)).clicked() {
                        do_import = true;
                    }
                    let can_export =
                        !busy && (self.total.unwrap_or(0) > 0 || !self.rows.is_empty());
                    if ui
                        .add_enabled(can_export, egui::Button::new(t.export))
                        .clicked()
                    {
                        do_export = true;
                    }
                    ui.separator();
                    if ui
                        .selectable_label(self.active_tab == AppTab::Results, t.results_tab)
                        .clicked()
                    {
                        self.active_tab = AppTab::Results;
                    }
                    if ui
                        .selectable_label(self.active_tab == AppTab::Analytics, t.analytics)
                        .clicked()
                    {
                        switched_to_analytics = self.active_tab != AppTab::Analytics;
                        self.active_tab = AppTab::Analytics;
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button(t.columns_btn, |ui| {
                            for (i, name) in RESULT_COLUMNS.iter().enumerate() {
                                let mut v = self.visible_cols[i];
                                if ui.checkbox(&mut v, header_for(name)).changed() {
                                    let visible_count =
                                        self.visible_cols.iter().filter(|x| **x).count();
                                    if v || visible_count > 1 {
                                        self.visible_cols[i] = v;
                                        self.persist_hidden_cols();
                                    }
                                }
                            }
                        });
                        let filters_btn = ui.selectable_label(self.show_filters, t.filters);
                        if filters_btn.clicked() {
                            self.show_filters = !self.show_filters;
                        }
                        let questions_resp = ui
                            .menu_button(guided_questions_label(self.lang), |ui| {
                                if guided_items.is_empty() || guided_query.is_empty() {
                                    ui.label(
                                        egui::RichText::new(guided_questions_empty(self.lang))
                                            .weak(),
                                    );
                                    return;
                                }
                                ui.label(
                                    egui::RichText::new(query_summary(&guided_query, t))
                                        .weak()
                                        .small(),
                                );
                                let mut current_section: Option<GuidedQuestionSection> = None;
                                for (section, kind) in &guided_items {
                                    if current_section != Some(*section) {
                                        if current_section.is_some() {
                                            ui.separator();
                                        }
                                        current_section = Some(*section);
                                        ui.label(
                                            egui::RichText::new(guided_section_title(
                                                *section, self.lang,
                                            ))
                                            .strong(),
                                        );
                                    }
                                    let Some(action) = guided_question_action(
                                        *kind,
                                        &guided_text,
                                        &guided_filters,
                                    ) else {
                                        continue;
                                    };
                                    if ui.button(guided_question_title(*kind, self.lang)).clicked()
                                    {
                                        guided_action = Some(action);
                                        ui.close();
                                    }
                                }
                            })
                            .response;
                        questions_resp.on_hover_text(guided_questions_hover(self.lang));
                        let saved_resp = ui
                            .menu_button("\u{2605}", |ui| {
                                if ui.button(save_current_query_label(self.lang)).clicked() {
                                    save_current_query = true;
                                    ui.close();
                                }
                                ui.separator();
                                if saved_queries.is_empty() {
                                    ui.label(
                                        egui::RichText::new(empty_saved_queries_label(self.lang))
                                            .weak(),
                                    );
                                } else {
                                    for (idx, item) in saved_queries.iter().enumerate() {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .button(trunc_label(&item.name, 56))
                                                .on_hover_text(query_summary(&item.query, t))
                                                .clicked()
                                            {
                                                apply_stored_query = Some(item.query.clone());
                                                ui.close();
                                            }
                                            if ui.small_button("\u{00D7}").clicked() {
                                                remove_saved_query = Some(idx);
                                                ui.close();
                                            }
                                        });
                                    }
                                }
                            })
                            .response;
                        saved_resp.on_hover_text(saved_queries_label(self.lang));
                        let recent_resp = ui
                            .menu_button("\u{21BA}", |ui| {
                                if recent_queries.is_empty() {
                                    ui.label(
                                        egui::RichText::new(empty_recent_queries_label(self.lang))
                                            .weak(),
                                    );
                                } else {
                                    for item in &recent_queries {
                                        if ui
                                            .button(trunc_label(&item.name, 64))
                                            .on_hover_text(query_summary(&item.query, t))
                                            .clicked()
                                        {
                                            apply_stored_query = Some(item.query.clone());
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button(clear_recent_queries_label(self.lang)).clicked() {
                                        clear_recent_queries = true;
                                        ui.close();
                                    }
                                }
                            })
                            .response;
                        recent_resp.on_hover_text(recent_queries_label(self.lang));
                        let find_btn = egui::Button::new(
                            egui::RichText::new(t.find).color(egui::Color32::WHITE),
                        )
                        .fill(ACCENT);
                        if ui.add(find_btn).clicked() {
                            do_search = true;
                        }
                        let edit = egui::TextEdit::singleline(&mut self.query_text)
                            .hint_text(t.search_hint)
                            .desired_width(ui.available_width());
                        let response = ui.add(edit);
                        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            do_search = true;
                        }
                        if response.changed()
                            && self.query_text.trim().is_empty()
                            && !self.active_query.text.trim().is_empty()
                        {
                            do_search = true;
                        }
                    });
                });

                if self.show_filters {
                    ui.add_space(6.0);
                    if self.ui_filters(ui) {
                        do_search = true;
                    }
                }
                ui.add_space(2.0);
            });
        if save_current_query {
            self.save_current_query();
        }
        if let Some(index) = remove_saved_query {
            self.remove_saved_query(index);
        }
        if clear_recent_queries {
            self.clear_recent_queries();
        }
        if let Some(query) = apply_stored_query {
            self.apply_stored_query(query);
        } else if let Some(action) = guided_action {
            self.run_guided_question(action);
        } else if do_search {
            self.start_search(true);
        } else if switched_to_analytics {
            self.request_analytics();
        }
        if do_import {
            self.pick_and_import(&ctx);
        }
        if do_export {
            self.pick_and_export(&ctx);
        }
    }

    /// Renders filters and returns true when a search should be started.
    fn ui_filters(&mut self, ui: &mut egui::Ui) -> bool {
        let t = self.t();
        let mut search = false;
        ui.horizontal_wrapped(|ui| {
            filter_field(ui, t.year, &mut self.filters.year, 60.0, &mut search);
            filter_field(
                ui,
                t.product_code,
                &mut self.filters.product_code,
                110.0,
                &mut search,
            );
            filter_field(ui, t.edrpou, &mut self.filters.edrpou, 100.0, &mut search);
            filter_field(
                ui,
                t.trademark,
                &mut self.filters.trademark,
                120.0,
                &mut search,
            );
            filter_field(ui, t.sender, &mut self.filters.sender, 180.0, &mut search);
            filter_field(
                ui,
                t.recipient,
                &mut self.filters.recipient,
                180.0,
                &mut search,
            );
            filter_field(
                ui,
                t.description,
                &mut self.filters.description,
                180.0,
                &mut search,
            );
            filter_field(
                ui,
                t.trade_country,
                &mut self.filters.trade_country,
                80.0,
                &mut search,
            );
            filter_field(
                ui,
                t.dispatch_country,
                &mut self.filters.dispatch_country,
                80.0,
                &mut search,
            );
            filter_field(
                ui,
                t.origin_country,
                &mut self.filters.origin_country,
                80.0,
                &mut search,
            );
            ui.vertical(|ui| {
                ui.label(" ");
                if ui.button(t.clear_filters).clicked() {
                    self.filters.clear();
                    search = true;
                }
            });
        });
        search
    }

    fn ui_status_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status").show_inside(root, |ui| {
            ui.add_space(4.0);
            if self.op.is_some() {
                self.ui_progress(ui);
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                if self.search_in_flight {
                    ui.spinner();
                    ui.label(self.t().searching);
                } else if !self.status.text.is_empty() {
                    let color = if self.status.is_error {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.colored_label(color, &self.status.text);
                } else if let Some(ms) = self.last_search_ms {
                    let start = self.page * PAGE_SIZE + 1;
                    let end = self.page * PAGE_SIZE + self.rows.len() as u64;
                    if let Some(total) = self.total {
                        if total > 0 {
                            let mut text = fmt(
                                self.t().rows_of,
                                &[
                                    &group_digits(start),
                                    &group_digits(end.min(total)),
                                    &group_digits(total),
                                ],
                            );
                            text.push_str("  \u{00B7}  ");
                            text.push_str(&fmt(self.t().search_ms, &[&ms.to_string()]));
                            if self.selected.len() > 1 {
                                text.push_str("  \u{00B7}  ");
                                text.push_str(&fmt(
                                    self.t().selected_n,
                                    &[&self.selected.len().to_string()],
                                ));
                            }
                            ui.label(text);
                        }
                    } else if !self.rows.is_empty() {
                        let mut text = fmt(
                            self.t().rows_of,
                            &[&group_digits(start), &group_digits(end), "?"],
                        );
                        text.push_str("  \u{00B7}  ");
                        text.push_str(&fmt(self.t().search_ms, &[&ms.to_string()]));
                        if self.count_in_flight {
                            text.push_str("  \u{00B7}  ");
                            text.push_str(self.t().searching);
                        }
                        if self.selected.len() > 1 {
                            text.push_str("  \u{00B7}  ");
                            text.push_str(&fmt(
                                self.t().selected_n,
                                &[&self.selected.len().to_string()],
                            ));
                        }
                        ui.label(text);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.ui_pagination(ui);
                });
            });
            ui.add_space(4.0);
        });
    }

    fn ui_progress(&mut self, ui: &mut egui::Ui) {
        let Some(op) = &self.op else { return };
        let t = self.t();
        let mut cancel_clicked = false;
        ui.horizontal(|ui| {
            match op.kind {
                OpKind::Clear => {
                    ui.spinner();
                    ui.label(t.clearing);
                }
                OpKind::Export => {
                    let (done, total) = op.export_progress;
                    ui.label(t.exporting);
                    let frac = if total > 0 {
                        done as f32 / total as f32
                    } else {
                        0.0
                    };
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_width(ui.available_width() - 110.0)
                            .text(format!("{} / {}", group_digits(done), group_digits(total))),
                    );
                }
                OpKind::Import => {
                    if let Some(ev) = &op.last_event {
                        let phase = match ev.phase {
                            ImportPhase::Reading => t.reading_file,
                            ImportPhase::Inserting => t.writing_rows,
                            ImportPhase::Indexing => t.indexing,
                        };
                        let label = if ev.file_name.is_empty() {
                            phase.to_string()
                        } else {
                            format!(
                                "{} \u{00B7} {}",
                                fmt(
                                    t.file_of,
                                    &[
                                        &ev.file_idx.to_string(),
                                        &ev.file_count.to_string(),
                                        &ev.file_name
                                    ]
                                ),
                                phase
                            )
                        };
                        ui.label(label);
                        if ev.total > 0 {
                            let frac = ev.done as f32 / ev.total as f32;
                            ui.add(
                                egui::ProgressBar::new(frac)
                                    .desired_width(ui.available_width() - 110.0)
                                    .text(format!(
                                        "{} / {}",
                                        group_digits(ev.done),
                                        group_digits(ev.total)
                                    )),
                            );
                        } else {
                            ui.spinner();
                            if ev.done > 0 {
                                ui.label(group_digits(ev.done));
                            }
                        }
                    } else {
                        ui.spinner();
                        ui.label(t.reading_file);
                    }
                }
            }
            if op.kind != OpKind::Clear {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(t.cancel).clicked() {
                        cancel_clicked = true;
                    }
                });
            }
        });
        if cancel_clicked && let Some(op) = &self.op {
            op.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn ui_pagination(&mut self, ui: &mut egui::Ui) {
        let pages = self.page_count();
        let page = self.page;
        let can_go_next = self
            .total
            .map(|_| page + 1 < pages)
            .unwrap_or(!self.search_in_flight && self.page_has_next);
        let mut goto: Option<u64> = None;
        // right_to_left draws from the end.
        if ui
            .add_enabled(
                self.total.is_some() && page + 1 < pages,
                egui::Button::new("⏭"),
            )
            .clicked()
        {
            goto = Some(pages - 1);
        }
        if ui
            .add_enabled(can_go_next, egui::Button::new("▶"))
            .clicked()
        {
            goto = Some(page + 1);
        }
        let page_total = self
            .total
            .map(|_| group_digits(pages))
            .unwrap_or_else(|| "?".to_string());
        ui.label(format!("{} / {}", group_digits(page + 1), page_total));
        if ui.add_enabled(page > 0, egui::Button::new("◀")).clicked() {
            goto = Some(page - 1);
        }
        if ui.add_enabled(page > 0, egui::Button::new("⏮")).clicked() {
            goto = Some(0);
        }
        if let Some(p) = goto {
            self.goto_page(p);
        }
    }

    fn ui_analytics_view(&mut self, root: &mut egui::Ui) {
        let mut need_request = false;
        egui::CentralPanel::default().show_inside(root, |ui| {
            let t = self.t();
            if self.active_query.is_empty() {
                ui.add_space((ui.available_height() * 0.30).max(0.0));
                ui.vertical_centered(|ui| {
                    ui.heading(t.analytics);
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(t.analytics_hint).weak());
                });
                return;
            }

            let Some(analytics) = &self.analytics else {
                need_request = !self.analytics_loading;
                ui.add_space((ui.available_height() * 0.30).max(0.0));
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.add_space(8.0);
                    ui.label(t.searching);
                });
                return;
            };

            let mut action: Option<AnalyticsFilterAction> = None;
            let mut card_action: Option<AnalyticsCardAction> = None;
            let mut show_more = false;
            let mut new_metric: Option<MonthMetric> = None;
            let mut new_view: Option<AnalyticsView> = None;
            let mut new_hs: Option<u8> = None;
            let mut new_pivot_row: Option<PivotDim> = None;
            let mut new_pivot_col: Option<PivotDim> = None;
            let mut new_pivot_metric: Option<PivotMetric> = None;
            let mut copy_pivot = false;
            let mut scan_underpricing = false;
            let mut open_card_id: Option<i64> = None;
            let p_row = self.pivot_row_dim;
            let p_col = self.pivot_col_dim;
            let p_metric = self.pivot_metric;
            let month_metric = self.month_metric;
            let view = self.analytics_view;
            let view_ready = self.analytics_loaded[view.index()];
            let loading = self.analytics_loading;
            let lang = self.lang;
            let hs_level = self.hs_level;

            // Analytics sub-tabs: each one is a focused screen.
            ui.horizontal(|ui| {
                for v in AnalyticsView::ALL {
                    let label = match v {
                        AnalyticsView::Overview => t.tab_overview,
                        AnalyticsView::Companies => t.companies_section,
                        AnalyticsView::Products => t.products_section,
                        AnalyticsView::Countries => t.countries_section,
                        AnalyticsView::Prices => t.prices_section,
                        AnalyticsView::Pivot => t.tab_pivot,
                    };
                    if ui.selectable_label(view == v, label).clicked() && v != view {
                        new_view = Some(v);
                    }
                }
                if loading || self.search_in_flight {
                    ui.spinner();
                }
                if matches!(
                    view,
                    AnalyticsView::Companies | AnalyticsView::Products | AnalyticsView::Countries
                ) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.analytics_limit < 50 && ui.button(t.show_more).clicked() {
                            show_more = true;
                        }
                        let shown = self.analytics_limit.min(50);
                        ui.label(
                            egui::RichText::new(fmt(t.showing_top, &[&shown.to_string()])).weak(),
                        );
                    });
                }
            });
            // One-line summary keeps context visible on every sub-tab.
            ui.horizontal(|ui| {
                let summary = ui.label(
                    egui::RichText::new(fmt(
                        t.mini_summary,
                        &[
                            &group_digits(analytics.overview.row_count),
                            &fmt_compact(analytics.overview.total_value_usd),
                            &fmt_compact(analytics.overview.total_net_kg),
                        ],
                    ))
                    .weak()
                    .small(),
                );
                summary.on_hover_text(analytics_calc_short_note(lang));
                if let (Some(first), Some(last)) =
                    (analytics.months.first(), analytics.months.last())
                {
                    ui.label(
                        egui::RichText::new(fmt(
                            t.period_of,
                            &[
                                &first.month,
                                &last.month,
                                &analytics.months.len().to_string(),
                            ],
                        ))
                        .weak()
                        .small(),
                    );
                }
            });
            egui::CollapsingHeader::new(analytics_calc_title(lang))
                .id_salt("analytics_calculation_notes")
                .show(ui, |ui| {
                    for line in analytics_calc_lines(lang) {
                        ui.label(egui::RichText::new(*line).weak().small());
                    }
                });
            ui.add_space(8.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                match view {
                    AnalyticsView::Overview => {
                        ui.label(egui::RichText::new(t.analytics_scope_note).weak().small());
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            kpi_tile(
                                ui,
                                t.rows_label,
                                group_digits(analytics.overview.row_count),
                                t.rows_help,
                            );
                            kpi_tile(
                                ui,
                                t.declarations_label,
                                group_digits(analytics.overview.declaration_count),
                                t.declarations_help,
                            );
                            kpi_tile(
                                ui,
                                t.recipients_label,
                                group_digits(analytics.overview.distinct_recipients),
                                t.recipients_help,
                            );
                            kpi_tile(
                                ui,
                                t.total_value,
                                fmt_compact(analytics.overview.total_value_usd),
                                t.total_value_help,
                            );
                            kpi_tile(
                                ui,
                                t.net_weight,
                                format!("{} kg", fmt_compact(analytics.overview.total_net_kg)),
                                t.net_weight_help,
                            );
                            kpi_tile(
                                ui,
                                t.avg_value_kg,
                                fmt_decimal(analytics.overview.avg_value_per_net_kg, 2),
                                t.avg_value_kg_help,
                            );
                            kpi_tile(
                                ui,
                                t.product_codes_count,
                                group_digits(analytics.overview.distinct_product_codes),
                                t.product_codes_help,
                            );
                            kpi_tile(
                                ui,
                                t.countries_count,
                                group_digits(analytics.overview.distinct_origin_countries),
                                t.countries_help,
                            );
                        });
                        ui.add_space(12.0);
                        if !analytics.months.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(t.months_section).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        for (metric, label) in [
                                            (MonthMetric::AvgPrice, t.metric_price),
                                            (MonthMetric::NetWeight, t.metric_weight),
                                            (MonthMetric::Rows, t.metric_rows),
                                            (MonthMetric::Value, t.metric_value),
                                        ] {
                                            if ui
                                                .selectable_label(month_metric == metric, label)
                                                .clicked()
                                            {
                                                new_metric = Some(metric);
                                            }
                                        }
                                    },
                                );
                            });
                            ui.label(egui::RichText::new(t.months_hint).weak().small());
                            ui.add_space(2.0);
                            months_chart(ui, &analytics.months, month_metric, lang);
                        }
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(t.currency_note).weak().small());
                    }
                    AnalyticsView::Companies | AnalyticsView::Countries => {
                        let (sections, hint) = if view == AnalyticsView::Companies {
                            (&analytics.company_sections, t.companies_section_hint)
                        } else {
                            (&analytics.country_sections, t.countries_section_hint)
                        };
                        ui.label(egui::RichText::new(hint).weak().small());
                        ui.add_space(6.0);
                        if !view_ready {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| {
                                ui.spinner();
                            });
                        } else if let Some(next) = analytics_cards(ui, sections, lang) {
                            card_action = Some(next);
                        }
                    }
                    AnalyticsView::Products => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(t.products_section_hint).weak().small());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    for (level, label) in
                                        [(10u8, t.hs_full), (6, "6"), (4, "4"), (2, "2")]
                                    {
                                        if ui.selectable_label(hs_level == level, label).clicked()
                                            && level != hs_level
                                        {
                                            new_hs = Some(level);
                                        }
                                    }
                                    ui.label(egui::RichText::new(t.hs_level_label).weak().small());
                                },
                            );
                        });
                        ui.add_space(6.0);
                        if !view_ready {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| {
                                ui.spinner();
                            });
                        } else if let Some(next) =
                            analytics_cards(ui, &analytics.product_sections, lang)
                        {
                            card_action = Some(next);
                        }
                    }
                    AnalyticsView::Prices => {
                        ui.label(egui::RichText::new(t.prices_section_hint).weak().small());
                        ui.add_space(6.0);
                        if !view_ready {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| {
                                ui.spinner();
                            });
                        } else {
                            price_table(ui, &analytics.price_sections, lang);
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(t.currency_note).weak().small());

                            ui.add_space(14.0);
                            ui.separator();
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(t.underpricing_title).strong());
                            ui.label(egui::RichText::new(t.underpricing_hint).weak().small());
                            ui.add_space(6.0);
                            match &self.underpricing {
                                _ if self.underpricing_loading => {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(t.searching);
                                    });
                                }
                                Some(uv) => {
                                    if let Some(id) =
                                        underpricing_table(ui, uv, lang, &mut scan_underpricing)
                                    {
                                        open_card_id = Some(id);
                                    }
                                }
                                None => {
                                    if ui
                                        .button(format!("\u{1F6A9} {}", t.underpricing_scan))
                                        .clicked()
                                    {
                                        scan_underpricing = true;
                                    }
                                }
                            }
                        }
                    }
                    AnalyticsView::Pivot => {
                        ui.label(egui::RichText::new(t.pivot_hint).weak().small());
                        ui.add_space(6.0);
                        // Dimension and metric selectors.
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(t.pivot_rows).strong());
                            pivot_dim_combo(ui, "pv_row", p_row, lang, &mut new_pivot_row);
                            ui.separator();
                            ui.label(egui::RichText::new(t.pivot_cols).strong());
                            pivot_dim_combo(ui, "pv_col", p_col, lang, &mut new_pivot_col);
                            ui.separator();
                            ui.label(egui::RichText::new(t.pivot_metric_label).strong());
                            for (m, label) in [
                                (PivotMetric::Value, t.metric_value),
                                (PivotMetric::Rows, t.metric_rows),
                                (PivotMetric::NetKg, t.metric_weight),
                            ] {
                                if ui.selectable_label(p_metric == m, label).clicked()
                                    && p_metric != m
                                {
                                    new_pivot_metric = Some(m);
                                }
                            }
                        });
                        ui.add_space(6.0);
                        match &self.pivot {
                            Some(pivot) if self.analytics_loaded[AnalyticsView::Pivot.index()] => {
                                if pivot.row_labels.is_empty() {
                                    ui.add_space(16.0);
                                    ui.label(egui::RichText::new(t.nothing_found).weak());
                                } else {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .small_button(format!("\u{29C9} {}", t.copy_all))
                                            .on_hover_text(copy_table_hover(lang))
                                            .clicked()
                                        {
                                            copy_pivot = true;
                                        }
                                    });
                                    ui.add_space(4.0);
                                    if let Some(next) =
                                        pivot_table_ui(ui, pivot, p_row, p_col, p_metric, lang)
                                    {
                                        action = Some(next);
                                    }
                                }
                            }
                            _ => {
                                ui.add_space(24.0);
                                ui.vertical_centered(|ui| {
                                    ui.spinner();
                                });
                            }
                        }
                    }
                }
                ui.add_space(8.0);
            });

            if let Some(metric) = new_metric {
                self.month_metric = metric;
            }
            if let Some(v) = new_view {
                self.analytics_view = v;
                need_request = true;
            }
            if let Some(level) = new_hs {
                self.hs_level = level;
                self.analytics_loaded[AnalyticsView::Products.index()] = false;
                need_request = true;
            }
            let mut repivot = false;
            if let Some(d) = new_pivot_row {
                self.pivot_row_dim = d;
                repivot = true;
            }
            if let Some(d) = new_pivot_col {
                self.pivot_col_dim = d;
                repivot = true;
            }
            if let Some(m) = new_pivot_metric {
                self.pivot_metric = m;
                repivot = true;
            }
            if repivot {
                self.request_pivot();
            }
            if copy_pivot && let Some(pivot) = &self.pivot {
                let tsv = pivot_tsv(pivot, self.pivot_row_dim, self.pivot_col_dim, self.lang);
                ui.ctx().copy_text(tsv);
            }
            if scan_underpricing {
                self.request_underpricing();
            }
            if let Some(id) = open_card_id {
                self.open_card_by_id(id);
            }
            if show_more {
                self.analytics_limit = 50;
                self.analytics_loaded = [false; 6];
                need_request = true;
            }
            if let Some(action) = action {
                self.apply_analytics_filter(action);
            }
            if let Some(card_action) = card_action {
                match card_action {
                    AnalyticsCardAction::Filter(action) => self.apply_analytics_filter(action),
                    AnalyticsCardAction::Explore(kind) => self.open_group_explorer(kind),
                }
            }
        });
        if need_request {
            self.request_analytics();
        }
    }

    fn ui_table(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(root, |ui| {
            if self.rows.is_empty() {
                let text = match self.total {
                    Some(0) if self.active_query.is_empty() => self.t().db_empty,
                    Some(0) => self.t().nothing_found,
                    None if self.search_in_flight || self.count_in_flight => self.t().searching,
                    _ => self.t().enter_query_hint,
                };
                ui.add_space((ui.available_height() * 0.35).max(0.0));
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("🔍").size(42.0).weak());
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(text).size(16.0).weak());
                });
                return;
            }
            let visible: Vec<usize> = (0..RESULT_COLUMNS.len())
                .filter(|i| self.visible_cols[*i])
                .collect();
            // Read modifiers from the click event itself: the keyboard state at
            // frame time may no longer contain Shift/Ctrl.
            let modifiers = ui.input(|i| {
                i.events
                    .iter()
                    .rev()
                    .find_map(|e| match e {
                        egui::Event::PointerButton {
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers,
                            ..
                        } => Some(*modifiers),
                        _ => None,
                    })
                    .unwrap_or(i.modifiers)
            });
            let dark = ui.visuals().dark_mode;
            let code_color = if dark {
                egui::Color32::from_rgb(132, 170, 255)
            } else {
                ACCENT
            };
            // Duplicate rows (already seen in an earlier file) are tinted amber.
            let dup_color = if dark {
                egui::Color32::from_rgb(235, 170, 90)
            } else {
                egui::Color32::from_rgb(160, 90, 0)
            };
            let mut open_card: Option<usize> = None;
            let mut clicked_row: Option<usize> = None;
            let mut menu_action: Option<RowMenuAction> = None;
            let t = self.t();
            let n_selected = self.selected.len();
            let text_height = egui::TextStyle::Body.resolve(ui.style()).size + 9.0;
            egui::ScrollArea::horizontal().show(ui, |ui| {
                let mut table = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(0.0);
                for idx in &visible {
                    let (width, _) = col_spec(RESULT_COLUMNS[*idx]);
                    table = table.column(Column::initial(width).at_least(40.0).clip(true));
                }
                table
                    .header(28.0, |mut header| {
                        for idx in &visible {
                            let name = RESULT_COLUMNS[*idx];
                            let (_, kind) = col_spec(name);
                            header.col(|ui| {
                                let resp = if kind == CellKind::Number {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| ui.strong(header_for(name)),
                                    )
                                    .inner
                                } else {
                                    ui.strong(header_for(name))
                                };
                                // Hover hint with the expanded meaning for
                                // abbreviated customs value/price columns.
                                if let Some(glossary) = column_glossary(name) {
                                    resp.on_hover_text(glossary);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(text_height, self.rows.len(), |mut row| {
                            let i = row.index();
                            row.set_selected(self.selected.contains(&i));
                            // A kept duplicate: the file where the row first
                            // appeared, used to tint and to fill the tooltip.
                            let dup_first = self.result_dups.get(i).and_then(|d| d.clone());
                            let is_dup = dup_first.is_some();
                            let mut clicked = false;
                            let mut double = false;
                            for idx in &visible {
                                let value = &self.rows[i][*idx];
                                let (_, kind) = col_spec(RESULT_COLUMNS[*idx]);
                                let (_, response) = row.col(|ui| {
                                    let rich = match kind {
                                        CellKind::Normal => egui::RichText::new(value),
                                        CellKind::Weak => egui::RichText::new(value).weak(),
                                        CellKind::Code => {
                                            egui::RichText::new(value).monospace().color(code_color)
                                        }
                                        CellKind::Number => egui::RichText::new(value).monospace(),
                                    };
                                    let rich = if is_dup { rich.color(dup_color) } else { rich };
                                    let label = egui::Label::new(rich).selectable(false).truncate();
                                    if kind == CellKind::Number {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.add(label);
                                            },
                                        );
                                    } else {
                                        ui.add(label);
                                    }
                                });
                                let response = if let Some(first_file) = &dup_first {
                                    response.on_hover_text(fmt(t.dup_first_seen, &[first_file]))
                                } else {
                                    response
                                };
                                clicked |= response.clicked();
                                double |= response.double_clicked();
                                response.context_menu(|ui| {
                                    let cells = &self.rows[i];
                                    if n_selected > 1
                                        && ui
                                            .button(fmt(
                                                t.copy_selected,
                                                &[&n_selected.to_string()],
                                            ))
                                            .clicked()
                                    {
                                        menu_action = Some(RowMenuAction::CopySelected);
                                        ui.close();
                                    }
                                    if ui.button(t.copy_value).clicked() {
                                        menu_action = Some(RowMenuAction::CopyCell(value.clone()));
                                        ui.close();
                                    }
                                    if ui.button(t.copy_row).clicked() {
                                        menu_action = Some(RowMenuAction::CopyRow(i));
                                        ui.close();
                                    }
                                    ui.separator();
                                    // Company profile by the row EDRPOU.
                                    if let Some(col) = result_col_index("edrpou") {
                                        let edrpou = cells[col].trim();
                                        if !edrpou.is_empty()
                                            && ui
                                                .button(format!(
                                                    "\u{1F3E2} {}: {}",
                                                    t.open_profile, edrpou
                                                ))
                                                .clicked()
                                        {
                                            menu_action = Some(RowMenuAction::OpenProfile(
                                                edrpou.to_string(),
                                            ));
                                            ui.close();
                                        }
                                    }
                                    let quick: [QuickAction; 4] = [
                                        (t.flt_sender, "sender", RowMenuAction::FilterSender),
                                        (
                                            t.flt_recipient,
                                            "recipient",
                                            RowMenuAction::FilterRecipient,
                                        ),
                                        (t.flt_code, "product_code", RowMenuAction::FilterCode),
                                        (t.flt_edrpou, "edrpou", RowMenuAction::FilterEdrpou),
                                    ];
                                    for (label, column, make) in quick {
                                        let Some(col) = result_col_index(column) else {
                                            continue;
                                        };
                                        let cell = cells[col].trim();
                                        if cell.is_empty() {
                                            continue;
                                        }
                                        let text = format!("{label}: {}", trunc_label(cell, 24));
                                        if ui.button(text).clicked() {
                                            menu_action = Some(make(cell.to_string()));
                                            ui.close();
                                        }
                                    }
                                });
                            }
                            if double {
                                open_card = Some(i);
                            } else if clicked {
                                clicked_row = Some(i);
                            }
                        });
                    });
            });
            if let Some(i) = clicked_row {
                self.handle_row_click(i, modifiers);
            }
            if let Some(i) = open_card {
                self.selected.clear();
                self.selected.insert(i);
                self.select_anchor = Some(i);
                self.open_card(i);
            }
            if let Some(action) = menu_action {
                let ctx = ui.ctx().clone();
                self.apply_menu_action(&ctx, action);
            }
        });
    }

    fn ui_card_window(&mut self, ctx: &egui::Context) {
        if !self.card_open {
            return;
        }
        let t = self.t();
        let mut open = self.card_open;
        if let Some(card) = &self.card {
            egui::Window::new(t.details)
                .open(&mut open)
                .default_size([640.0, 660.0])
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{}: {}", t.file_col, card.source_file))
                                .weak(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(t.copy_all).clicked() {
                                let text: String = card
                                    .fields
                                    .iter()
                                    .filter(|(_, v)| !v.is_empty())
                                    .map(|(h, v)| format!("{h}: {v}"))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                ctx.copy_text(text);
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        egui::Grid::new("card_grid")
                            .num_columns(2)
                            .striped(true)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                for (header, value) in &card.fields {
                                    ui.label(egui::RichText::new(*header).strong());
                                    if value.is_empty() {
                                        ui.label(egui::RichText::new("\u{2014}").weak());
                                    } else {
                                        ui.add(egui::Label::new(value).wrap());
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                });
        }
        self.card_open = open;
        if !self.card_open {
            self.card = None;
        }
    }

    fn ui_profile_view(&mut self, root: &mut egui::Ui) {
        let mut close = false;
        let mut filter_all = false;
        let mut action: Option<AnalyticsFilterAction> = None;
        egui::CentralPanel::default().show_inside(root, |ui| {
            let t = self.t();
            // Header: back button + company identity.
            ui.horizontal(|ui| {
                if ui.button(format!("\u{2190} {}", t.profile_back)).clicked() {
                    close = true;
                }
                ui.heading(t.company_profile);
                if self.profile_loading {
                    ui.spinner();
                }
            });
            ui.add_space(4.0);

            let Some(profile) = &self.profile else {
                ui.add_space((ui.available_height() * 0.30).max(0.0));
                ui.vertical_centered(|ui| {
                    ui.spinner();
                });
                return;
            };
            let lang = self.lang;

            // Company names (variants) and EDRPOU.
            let primary = profile.names.first().cloned().unwrap_or_default();
            ui.label(
                egui::RichText::new(if primary.is_empty() {
                    profile.edrpou.clone()
                } else {
                    primary.clone()
                })
                .size(18.0)
                .strong(),
            );
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{}: {}", t.edrpou, profile.edrpou)).weak());
                if ui.small_button(t.show_results).clicked() {
                    filter_all = true;
                }
            });
            if profile.names.len() > 1 {
                ui.label(
                    egui::RichText::new(fmt(t.also_known_as, &[&profile.names[1..].join(" · ")]))
                        .weak()
                        .small(),
                );
            }
            ui.add_space(8.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Headline numbers for this company.
                ui.horizontal_wrapped(|ui| {
                    kpi_tile(
                        ui,
                        t.rows_label,
                        group_digits(profile.overview.row_count),
                        t.rows_help,
                    );
                    kpi_tile(
                        ui,
                        t.declarations_label,
                        group_digits(profile.overview.declaration_count),
                        t.declarations_help,
                    );
                    kpi_tile(
                        ui,
                        t.total_value,
                        fmt_compact(profile.overview.total_value_usd),
                        t.total_value_help,
                    );
                    kpi_tile(
                        ui,
                        t.net_weight,
                        format!("{} kg", fmt_compact(profile.overview.total_net_kg)),
                        t.net_weight_help,
                    );
                    kpi_tile(
                        ui,
                        t.avg_value_kg,
                        fmt_decimal(profile.overview.avg_value_per_net_kg, 2),
                        t.avg_value_kg_help,
                    );
                    kpi_tile(
                        ui,
                        t.product_codes_count,
                        group_digits(profile.overview.distinct_product_codes),
                        t.product_codes_help,
                    );
                    kpi_tile(
                        ui,
                        t.unique_senders,
                        group_digits(profile.overview.distinct_senders),
                        t.unique_senders,
                    );
                });
                ui.add_space(12.0);

                if !profile.months.is_empty() {
                    ui.label(egui::RichText::new(t.months_section).strong());
                    ui.add_space(2.0);
                    months_chart(ui, &profile.months, MonthMetric::Value, lang);
                    ui.add_space(12.0);
                }

                ui.label(egui::RichText::new(t.products_section).strong());
                ui.label(egui::RichText::new(t.products_section_hint).weak().small());
                if let Some(next) =
                    analytics_cards_with_options(ui, &profile.product_sections, lang, false)
                    && let AnalyticsCardAction::Filter(filter) = next
                {
                    action = Some(filter);
                }
                ui.add_space(8.0);

                ui.label(egui::RichText::new(t.companies_section).strong());
                let sender_section = [AnalyticsSection {
                    kind: AnalyticsSectionKind::Senders,
                    rows: profile.top_senders.clone(),
                }];
                if let Some(next) = analytics_cards_with_options(ui, &sender_section, lang, false)
                    && let AnalyticsCardAction::Filter(filter) = next
                {
                    action = Some(filter);
                }
                ui.add_space(8.0);

                ui.label(egui::RichText::new(t.countries_section).strong());
                ui.label(egui::RichText::new(t.countries_section_hint).weak().small());
                if let Some(next) =
                    analytics_cards_with_options(ui, &profile.country_sections, lang, false)
                    && let AnalyticsCardAction::Filter(filter) = next
                {
                    action = Some(filter);
                }
                ui.add_space(8.0);

                ui.label(egui::RichText::new(t.prices_section).strong());
                ui.label(egui::RichText::new(t.prices_section_hint).weak().small());
                price_table(ui, &profile.price_sections, lang);
                ui.add_space(8.0);
            });
        });
        if close {
            self.close_profile();
        }
        if filter_all {
            let edrpou = self.profile.as_ref().map(|p| p.edrpou.clone());
            if let Some(edrpou) = edrpou {
                self.close_profile();
                self.apply_analytics_filter(AnalyticsFilterAction {
                    field: AnalyticsFilterField::Edrpou,
                    value: edrpou,
                });
            }
        }
        if let Some(action) = action {
            // Drill from a dossier card into filtered results.
            self.close_profile();
            self.apply_analytics_filter(action);
        }
    }

    fn ui_group_explorer_window(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let t = self.t();
        let Some(explorer) = self.group_explorer.as_mut() else {
            return;
        };

        let mut open = true;
        let mut close = false;
        let mut filter_action: Option<AnalyticsFilterAction> = None;
        let title = group_explorer_title(explorer.kind, lang);
        egui::Window::new(title)
            .id(egui::Id::new(format!(
                "analytics_group_explorer_{:?}",
                explorer.kind
            )))
            .open(&mut open)
            .default_width(980.0)
            .default_height(620.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if explorer.loading {
                        ui.spinner();
                        ui.label(t.searching);
                    } else {
                        ui.label(egui::RichText::new(group_explorer_count(
                            explorer.rows.len() as u64,
                            explorer.rows.len() as u64 >= FULL_SECTION_LIMIT,
                            lang,
                        )));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(t.close).clicked() {
                            close = true;
                        }
                    });
                });
                ui.label(
                    egui::RichText::new(group_explorer_hint(lang))
                        .weak()
                        .small(),
                );
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut explorer.label_filter)
                        .hint_text(group_search_hint(lang))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);

                if explorer.loading {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                    });
                    return;
                }
                if explorer.rows.is_empty() {
                    ui.label(egui::RichText::new(t.nothing_found).weak());
                    return;
                }

                let needle = explorer.label_filter.trim().to_lowercase();
                let mut visible_rows: Vec<&AnalyticsGroupRow> = explorer
                    .rows
                    .iter()
                    .filter(|row| needle.is_empty() || row.label.to_lowercase().contains(&needle))
                    .collect();
                sort_group_rows(&mut visible_rows, explorer.sort, explorer.descending);

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(group_visible_count(
                            visible_rows.len() as u64,
                            explorer.rows.len() as u64,
                            explorer.rows.len() as u64 >= FULL_SECTION_LIMIT,
                            lang,
                        ))
                        .weak()
                        .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(format!("\u{29C9} {}", copy_visible_label(lang)))
                            .on_hover_text(copy_table_hover(lang))
                            .clicked()
                        {
                            ui.ctx().copy_text(group_rows_tsv(&visible_rows, lang));
                        }
                    });
                });
                ui.add_space(4.0);

                if visible_rows.is_empty() {
                    ui.label(egui::RichText::new(t.nothing_found).weak());
                } else if let Some(next) = group_explorer_table(
                    ui,
                    &visible_rows,
                    &mut explorer.sort,
                    &mut explorer.descending,
                    lang,
                ) {
                    filter_action = Some(next);
                }
            });

        if !open || close {
            self.group_explorer = None;
        }
        if let Some(action) = filter_action {
            self.group_explorer = None;
            self.apply_analytics_filter(action);
        }
    }

    fn ui_import_report(&mut self, ctx: &egui::Context) {
        let Some(report) = &self.import_report else {
            return;
        };
        let t = self.t();
        let mut open = true;
        egui::Window::new(t.import_report)
            .open(&mut open)
            .default_width(560.0)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("report_grid")
                    .num_columns(2)
                    .striped(true)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for s in report {
                            ui.label(egui::RichText::new(&s.file_name).strong());
                            if let Some(err) = &s.error {
                                let text = if let Some(cols) = err.strip_prefix("__MISSING__") {
                                    fmt(t.missing_cols, &[cols])
                                } else {
                                    err.clone()
                                };
                                ui.colored_label(ui.visuals().error_fg_color, text);
                            } else if let Some(previous) = &s.skipped_duplicate_of {
                                ui.label(
                                    egui::RichText::new(fmt(t.file_skipped, &[previous])).weak(),
                                );
                            } else {
                                let mut text = fmt(
                                    t.file_result,
                                    &[
                                        &group_digits(s.imported),
                                        &group_digits(s.duplicates),
                                        &format!("{:.1}", s.seconds),
                                    ],
                                );
                                if s.cancelled {
                                    text.push_str(" \u{00B7} ");
                                    text.push_str(t.cancelled);
                                }
                                ui.label(text);
                            }
                            ui.end_row();
                        }
                    });
            });
        if !open {
            self.import_report = None;
        }
    }

    fn ui_help_window(&mut self, ctx: &egui::Context) {
        if !self.show_help {
            return;
        }
        // Remember that the guide has been seen, so it won't auto-open again.
        self.persist("help_seen", "1");
        let t = self.t();
        let mut open = self.show_help;
        egui::Window::new(format!("? {}", t.help))
            .open(&mut open)
            .collapsible(false)
            .default_width(560.0)
            .default_height(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for section in help_sections(self.lang) {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(section.title).strong().size(15.0));
                        ui.add_space(2.0);
                        for item in section.items {
                            ui.horizontal_top(|ui| {
                                ui.label(egui::RichText::new("•").weak());
                                ui.label(*item);
                            });
                        }
                        ui.add_space(6.0);
                    }
                });
            });
        self.show_help = open;
    }

    fn ui_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let t = self.t();
        let mut open = true;
        egui::Window::new(format!("\u{2699} {}", t.settings))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("settings_grid")
                    .num_columns(2)
                    .spacing([24.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(t.language);
                        let mut lang = self.lang;
                        egui::ComboBox::from_id_salt("settings_lang")
                            .width(150.0)
                            .selected_text(lang.label())
                            .show_ui(ui, |ui| {
                                for l in Lang::ALL {
                                    ui.selectable_value(&mut lang, l, l.label());
                                }
                            });
                        if lang != self.lang {
                            self.lang = lang;
                            self.persist("lang", lang.code());
                        }
                        ui.end_row();

                        ui.label(t.theme_label);
                        ui.horizontal(|ui| {
                            let dark = ui.visuals().dark_mode;
                            if ui.selectable_label(!dark, t.theme_light).clicked() && dark {
                                ctx.set_theme(egui::Theme::Light);
                                self.persist("theme", "light");
                            }
                            if ui.selectable_label(dark, t.theme_dark).clicked() && !dark {
                                ctx.set_theme(egui::Theme::Dark);
                                self.persist("theme", "dark");
                            }
                        });
                        ui.end_row();

                        ui.label(t.zoom_label);
                        ui.horizontal(|ui| {
                            let zoom = ctx.zoom_factor();
                            let mut new_zoom = zoom;
                            if ui.button("\u{2212}").clicked() {
                                new_zoom = (zoom - 0.1).max(0.6);
                            }
                            ui.label(format!("{:.0}%", zoom * 100.0));
                            if ui.button("+").clicked() {
                                new_zoom = (zoom + 0.1).min(2.0);
                            }
                            if (new_zoom - zoom).abs() > f32::EPSILON {
                                ctx.set_zoom_factor(new_zoom);
                                self.persist("zoom", &format!("{new_zoom:.2}"));
                            }
                            ui.label(egui::RichText::new("Ctrl + / \u{2212}").weak().small());
                        });
                        ui.end_row();
                    });

                ui.separator();
                ui.label(egui::RichText::new(t.db_section).strong());
                ui.add_space(4.0);
                egui::Grid::new("settings_db_grid")
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(t.db_file_label).weak());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(self.db_path.display().to_string()).small(),
                            )
                            .wrap(),
                        );
                        ui.end_row();
                        ui.label(egui::RichText::new(t.db_size_label).weak());
                        let size = std::fs::metadata(&self.db_path)
                            .map(|m| m.len())
                            .unwrap_or(0);
                        ui.label(format!("{:.2} GB", size as f64 / (1u64 << 30) as f64));
                        ui.end_row();
                    });
                ui.add_space(8.0);
                let busy = self.op.is_some();
                let clear_btn =
                    egui::Button::new(egui::RichText::new(t.clear_db).color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(200, 50, 50));
                if ui.add_enabled(!busy, clear_btn).clicked() {
                    self.confirm_clear = true;
                }
                ui.add_space(6.0);
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{}: {APP_VERSION}", t.version_label))
                        .weak()
                        .small(),
                );
            });
        self.show_settings = open;
    }

    fn ui_confirm_clear(&mut self, ctx: &egui::Context) {
        if !self.confirm_clear {
            return;
        }
        let t = self.t();
        egui::Window::new(t.clear_db)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(t.clear_confirm);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let yes_btn = egui::Button::new(
                        egui::RichText::new(t.clear_yes).color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(200, 50, 50));
                    if ui.add(yes_btn).clicked() {
                        self.confirm_clear = false;
                        self.show_settings = false;
                        self.start_clear_db(ctx);
                    }
                    if ui.button(t.cancel).clicked() {
                        self.confirm_clear = false;
                    }
                });
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.drain_messages();
        // Ctrl+C copies selected rows when focus is not inside a text field.
        let copy_requested = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if copy_requested && !self.selected.is_empty() && !self.card_open {
            self.copy_selected_rows(&ctx);
        }
        self.ui_toolbar(root);
        self.ui_status_bar(root);
        if self.profile.is_some() || self.profile_loading {
            self.ui_profile_view(root);
        } else {
            match self.active_tab {
                AppTab::Results => self.ui_table(root),
                AppTab::Analytics => self.ui_analytics_view(root),
            }
        }
        self.ui_card_window(&ctx);
        self.ui_group_explorer_window(&ctx);
        self.ui_import_report(&ctx);
        self.ui_settings_window(&ctx);
        self.ui_help_window(&ctx);
        self.ui_confirm_clear(&ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.show_help = true;
        }
        // Safety repaint: refresh regularly while a background operation runs.
        if self.op.is_some()
            || self.search_in_flight
            || self.count_in_flight
            || self.analytics_loading
            || self.profile_loading
            || self.underpricing_loading
            || self
                .group_explorer
                .as_ref()
                .map(|explorer| explorer.loading)
                .unwrap_or(false)
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

/// Bar chart of monthly dynamics. Bars are drawn with the painter;
/// hovering a bar shows the full numbers for that month.
fn months_chart(ui: &mut egui::Ui, months: &[AnalyticsMonthRow], metric: MonthMetric, lang: Lang) {
    let height = 190.0;
    let width = ui.available_width().max(320.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let visuals = ui.visuals();
    let rounding = egui::CornerRadius::same(5);
    ui.painter().rect(
        rect,
        rounding,
        visuals.faint_bg_color,
        visuals.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    let max_value = months
        .iter()
        .map(|m| metric.of(m))
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let label_h = 18.0;
    let pad = 10.0;
    let plot = egui::Rect::from_min_max(
        egui::pos2(rect.left() + pad, rect.top() + pad),
        egui::pos2(rect.right() - pad, rect.bottom() - pad - label_h),
    );

    // Horizontal grid: quarter lines with weak value captions.
    let grid_font = egui::FontId::new(10.5, egui::FontFamily::Monospace);
    let grid_color = visuals.weak_text_color().gamma_multiply(0.5);
    for step in 1..=3 {
        let frac = step as f32 / 4.0;
        let y = plot.bottom() - plot.height() * frac;
        ui.painter().hline(
            plot.x_range(),
            y,
            egui::Stroke::new(0.5, grid_color.gamma_multiply(0.6)),
        );
        ui.painter().text(
            egui::pos2(plot.left(), y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            fmt_compact(max_value * frac as f64),
            grid_font.clone(),
            grid_color,
        );
    }

    let n = months.len().max(1);
    let slot = plot.width() / n as f32;
    let bar_w = (slot * 0.72).clamp(3.0, 64.0);
    let hover_x = response.hover_pos().map(|p| p.x);
    let mut hovered: Option<usize> = None;

    let bar_color = if visuals.dark_mode {
        egui::Color32::from_rgb(80, 140, 255)
    } else {
        ACCENT
    };
    let month_font = egui::FontId::new(10.5, egui::FontFamily::Monospace);
    let value_font = egui::FontId::new(10.5, egui::FontFamily::Monospace);
    // Month labels are thinned out so they do not overlap.
    let label_every = ((42.0 / slot).ceil() as usize).max(1);

    for (i, month) in months.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let value = metric.of(month);
        let bar_h = (plot.height() * (value / max_value) as f32).max(1.5);
        let bar = egui::Rect::from_min_max(
            egui::pos2(cx - bar_w / 2.0, plot.bottom() - bar_h),
            egui::pos2(cx + bar_w / 2.0, plot.bottom()),
        );
        let is_hovered = hover_x
            .map(|x| (x - cx).abs() <= slot / 2.0)
            .unwrap_or(false);
        if is_hovered {
            hovered = Some(i);
        }
        let color = if is_hovered {
            bar_color
        } else {
            bar_color.gamma_multiply(0.62)
        };
        ui.painter()
            .rect_filled(bar, egui::CornerRadius::same(2), color);
        if i % label_every == 0 {
            ui.painter().text(
                egui::pos2(cx, rect.bottom() - 4.0),
                egui::Align2::CENTER_BOTTOM,
                short_month(&month.month),
                month_font.clone(),
                visuals.weak_text_color(),
            );
        }
        // Draw the value above the bar when there is enough room.
        if slot >= 46.0 && value > 0.0 {
            ui.painter().text(
                egui::pos2(cx, bar.top() - 2.0),
                egui::Align2::CENTER_BOTTOM,
                fmt_compact(value),
                value_font.clone(),
                visuals.weak_text_color(),
            );
        }
    }

    if let Some(i) = hovered {
        let month = &months[i];
        let t = tr(lang);
        let (rows_l, decls_l, value_l, weight_l) = (
            t.chart_rows,
            t.chart_declarations,
            t.chart_value,
            t.chart_net_weight,
        );
        response.on_hover_text(format!(
            "{}\n{}: {}\n{}: {}\n{}: {}\n{}: {} kg",
            month.month,
            rows_l,
            group_digits(month.rows),
            decls_l,
            group_digits(month.declarations),
            value_l,
            fmt_decimal(month.total_value_usd, 0),
            weight_l,
            fmt_decimal(month.total_net_kg, 0),
        ));
    }
}

/// "2024-03" -> "03'24"
fn short_month(month: &str) -> String {
    match (month.get(0..4), month.get(5..7)) {
        (Some(year), Some(m)) => format!("{m}'{}", &year[2..]),
        _ => month.to_string(),
    }
}

/// Compact number for chart captions: 12.4M, 980K, 312.
fn fmt_compact(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1.0e9 {
        format!("{:.1}B", value / 1.0e9)
    } else if abs >= 1.0e6 {
        format!("{:.1}M", value / 1.0e6)
    } else if abs >= 1.0e4 {
        format!("{:.0}K", value / 1.0e3)
    } else if abs >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn kpi_tile(ui: &mut egui::Ui, label: &str, value: String, help: &str) {
    let frame = egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(8));
    let response = frame
        .show(ui, |ui| {
            ui.set_min_width(146.0);
            ui.label(egui::RichText::new(label).weak().small());
            ui.add_space(2.0);
            ui.label(egui::RichText::new(value).strong().monospace().size(16.0));
        })
        .response;
    response.on_hover_text(help);
}

/// Cards of one analytics scope, laid out side by side so the whole scope
/// fits on screen without endless scrolling.
fn analytics_cards(
    ui: &mut egui::Ui,
    sections: &[AnalyticsSection],
    lang: Lang,
) -> Option<AnalyticsCardAction> {
    analytics_cards_with_options(ui, sections, lang, true)
}

fn analytics_cards_with_options(
    ui: &mut egui::Ui,
    sections: &[AnalyticsSection],
    lang: Lang,
    allow_explore: bool,
) -> Option<AnalyticsCardAction> {
    let mut action = None;
    let sections: Vec<&AnalyticsSection> = sections.iter().filter(|s| !s.rows.is_empty()).collect();
    if sections.is_empty() {
        return None;
    }
    let gap = 10.0;
    let avail = ui.available_width();
    let per_row = if avail >= 960.0 {
        3.min(sections.len())
    } else if avail >= 640.0 {
        2.min(sections.len())
    } else {
        1
    };
    let card_w = ((avail - gap * (per_row as f32 - 1.0)) / per_row as f32).max(260.0);
    for chunk in sections.chunks(per_row) {
        ui.with_layout(
            egui::Layout::left_to_right(egui::Align::Min).with_main_align(egui::Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                for section in chunk {
                    ui.allocate_ui_with_layout(
                        egui::vec2(card_w, 10.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_width(card_w);
                            ui.set_max_width(card_w);
                            if let Some(next) = analytics_card(ui, section, lang, allow_explore) {
                                action = Some(next);
                            }
                        },
                    );
                }
            },
        );
        ui.add_space(gap);
    }
    action
}

/// Card rows as a TSV table that pastes directly into Excel.
fn section_tsv(section: &AnalyticsSection, lang: Lang) -> String {
    let rows: Vec<&AnalyticsGroupRow> = section.rows.iter().collect();
    group_rows_tsv(&rows, lang)
}

fn group_rows_tsv(rows: &[&AnalyticsGroupRow], lang: Lang) -> String {
    let header = tr(lang).group_tsv_header;
    let mut out = String::from(header);
    for row in rows {
        out.push('\n');
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}",
            row.label,
            row.rows,
            row.declarations,
            row.companies,
            row.total_value_usd,
            row.total_net_kg,
            row.total_gross_kg,
            row.total_quantity,
            row.share_percent,
            row.avg_value_per_net_kg
        ));
    }
    out
}

fn copy_table_hover(lang: Lang) -> &'static str {
    tr(lang).copy_table_hover
}

fn all_rows_button(lang: Lang) -> &'static str {
    tr(lang).all_label
}

fn all_rows_hover(lang: Lang) -> &'static str {
    tr(lang).all_rows_hover
}

fn analytics_card(
    ui: &mut egui::Ui,
    section: &AnalyticsSection,
    lang: Lang,
    allow_explore: bool,
) -> Option<AnalyticsCardAction> {
    let mut action = None;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(section_title(section.kind, lang)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("\u{29C9}")
                        .on_hover_text(copy_table_hover(lang))
                        .clicked()
                    {
                        ui.ctx().copy_text(section_tsv(section, lang));
                    }
                    if allow_explore
                        && ui
                            .small_button(all_rows_button(lang))
                            .on_hover_text(all_rows_hover(lang))
                            .clicked()
                    {
                        action = Some(AnalyticsCardAction::Explore(section.kind));
                    }
                });
            });
            ui.add_space(4.0);
            for row in &section.rows {
                if let Some(next) = compact_bar_row(ui, row, lang) {
                    action = Some(AnalyticsCardAction::Filter(next));
                }
            }
            let total_share: f64 = section.rows.iter().map(|r| r.share_percent).sum();
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(fmt(
                    top_share_pattern(lang),
                    &[
                        &section.rows.len().to_string(),
                        &fmt_decimal(total_share.min(100.0), 1),
                    ],
                ))
                .weak()
                .small(),
            );
        });
    action
}

/// One compact clickable row: label, share bar, value and percentage.
/// Full numbers are in the hover tooltip; click applies the filter.
fn compact_bar_row(
    ui: &mut egui::Ui,
    row: &AnalyticsGroupRow,
    lang: Lang,
) -> Option<AnalyticsFilterAction> {
    let width = ui.available_width();
    let height = 24.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let visuals = ui.visuals();
    let rounding = egui::CornerRadius::same(3);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, rounding, visuals.widgets.hovered.weak_bg_fill);
    }
    let share_width = (rect.width() * (row.share_percent as f32 / 100.0)).clamp(0.0, rect.width());
    let share_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.bottom() - 3.0),
        egui::vec2(share_width, 3.0),
    );
    let bar_bg = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.bottom() - 3.0),
        egui::vec2(rect.width(), 3.0),
    );
    let bar_color = if visuals.dark_mode {
        egui::Color32::from_rgb(80, 140, 255)
    } else {
        ACCENT
    };
    ui.painter()
        .rect_filled(bar_bg, rounding, visuals.faint_bg_color);
    ui.painter().rect_filled(share_rect, rounding, bar_color);

    let label_font = egui::FontId::new(12.5, egui::FontFamily::Proportional);
    let mono_font = egui::FontId::new(11.5, egui::FontFamily::Monospace);
    let right_text = format!(
        "{} · {}%",
        fmt_compact(row.total_value_usd),
        fmt_decimal(row.share_percent, 1)
    );
    let right_w = right_text.chars().count() as f32 * 7.0;
    ui.painter().text(
        egui::pos2(rect.left() + 2.0, rect.top() + 9.0),
        egui::Align2::LEFT_CENTER,
        trunc_label(
            &row.label,
            ((width - right_w - 12.0) / 6.8).max(8.0) as usize,
        ),
        label_font,
        visuals.text_color(),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 2.0, rect.top() + 9.0),
        egui::Align2::RIGHT_CENTER,
        right_text,
        mono_font,
        visuals.weak_text_color(),
    );

    let response = response.on_hover_text(row_hover_text(row, lang));
    if response.clicked() {
        row.filter_action.clone()
    } else {
        None
    }
}

fn group_explorer_table(
    ui: &mut egui::Ui,
    rows: &[&AnalyticsGroupRow],
    sort: &mut GroupSort,
    descending: &mut bool,
    lang: Lang,
) -> Option<AnalyticsFilterAction> {
    let t = tr(lang);
    let mut action = None;
    egui::ScrollArea::horizontal()
        .id_salt("group_explorer_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(270.0).at_least(140.0).clip(true))
                .column(Column::initial(82.0).at_least(64.0))
                .column(Column::initial(104.0).at_least(78.0))
                .column(Column::initial(94.0).at_least(70.0))
                .column(Column::initial(110.0).at_least(86.0))
                .column(Column::initial(98.0).at_least(74.0))
                .column(Column::initial(98.0).at_least(74.0))
                .column(Column::initial(96.0).at_least(72.0))
                .column(Column::initial(82.0).at_least(62.0))
                .column(Column::initial(96.0).at_least(72.0))
                .header(24.0, |mut header| {
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            label_header(lang),
                            GroupSort::Label,
                            sort,
                            descending,
                        );
                    });
                    header.col(|ui| {
                        sortable_group_header(ui, t.rows_label, GroupSort::Rows, sort, descending);
                    });
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            t.declarations_label,
                            GroupSort::Declarations,
                            sort,
                            descending,
                        );
                    });
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            companies_header(lang),
                            GroupSort::Companies,
                            sort,
                            descending,
                        );
                    });
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            t.total_value,
                            GroupSort::Value,
                            sort,
                            descending,
                        );
                    });
                    header.col(|ui| {
                        sortable_group_header(ui, t.net_weight, GroupSort::NetKg, sort, descending);
                    });
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            t.gross_weight,
                            GroupSort::GrossKg,
                            sort,
                            descending,
                        );
                    });
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            t.quantity,
                            GroupSort::Quantity,
                            sort,
                            descending,
                        );
                    });
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            share_header(lang),
                            GroupSort::Share,
                            sort,
                            descending,
                        );
                    });
                    header.col(|ui| {
                        sortable_group_header(
                            ui,
                            t.avg_value_kg,
                            GroupSort::ValuePerKg,
                            sort,
                            descending,
                        );
                    });
                })
                .body(|body| {
                    body.rows(24.0, rows.len(), |mut table_row| {
                        let row = rows[table_row.index()];
                        let mut clicked = false;
                        table_row.col(|ui| {
                            clicked |= group_text_cell(ui, &row.label, row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |= group_numeric_cell(ui, group_digits(row.rows), row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |=
                                group_numeric_cell(ui, group_digits(row.declarations), row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |=
                                group_numeric_cell(ui, group_digits(row.companies), row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |=
                                group_numeric_cell(ui, fmt_compact(row.total_value_usd), row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |=
                                group_numeric_cell(ui, fmt_compact(row.total_net_kg), row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |=
                                group_numeric_cell(ui, fmt_compact(row.total_gross_kg), row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |=
                                group_numeric_cell(ui, fmt_compact(row.total_quantity), row, lang);
                        });
                        table_row.col(|ui| {
                            clicked |= group_numeric_cell(
                                ui,
                                format!("{}%", fmt_decimal(row.share_percent, 1)),
                                row,
                                lang,
                            );
                        });
                        table_row.col(|ui| {
                            clicked |= group_numeric_cell(
                                ui,
                                fmt_decimal(row.avg_value_per_net_kg, 2),
                                row,
                                lang,
                            );
                        });
                        if clicked {
                            action = row.filter_action.clone();
                        }
                    });
                });
        });
    action
}

fn sortable_group_header(
    ui: &mut egui::Ui,
    label: &str,
    column: GroupSort,
    current: &mut GroupSort,
    descending: &mut bool,
) {
    let selected = *current == column;
    let arrow = if selected {
        if *descending { " ▼" } else { " ▲" }
    } else {
        ""
    };
    if ui.small_button(format!("{label}{arrow}")).clicked() {
        if selected {
            *descending = !*descending;
        } else {
            *current = column;
            *descending = column != GroupSort::Label;
        }
    }
}

fn group_text_cell(ui: &mut egui::Ui, text: &str, row: &AnalyticsGroupRow, lang: Lang) -> bool {
    ui.add(egui::Label::new(text).selectable(false).truncate())
        .on_hover_text(row_hover_text(row, lang))
        .clicked()
}

fn group_numeric_cell(
    ui: &mut egui::Ui,
    text: String,
    row: &AnalyticsGroupRow,
    lang: Lang,
) -> bool {
    ui.add(egui::Label::new(egui::RichText::new(text).monospace()).selectable(false))
        .on_hover_text(row_hover_text(row, lang))
        .clicked()
}

fn sort_group_rows(rows: &mut [&AnalyticsGroupRow], sort: GroupSort, descending: bool) {
    rows.sort_by(|a, b| {
        let ord = match sort {
            GroupSort::Label => a.label.to_lowercase().cmp(&b.label.to_lowercase()),
            GroupSort::Rows => a.rows.cmp(&b.rows),
            GroupSort::Declarations => a.declarations.cmp(&b.declarations),
            GroupSort::Companies => a.companies.cmp(&b.companies),
            GroupSort::Value => cmp_f64(a.total_value_usd, b.total_value_usd),
            GroupSort::NetKg => cmp_f64(a.total_net_kg, b.total_net_kg),
            GroupSort::GrossKg => cmp_f64(a.total_gross_kg, b.total_gross_kg),
            GroupSort::Quantity => cmp_f64(a.total_quantity, b.total_quantity),
            GroupSort::Share => cmp_f64(a.share_percent, b.share_percent),
            GroupSort::ValuePerKg => cmp_f64(a.avg_value_per_net_kg, b.avg_value_per_net_kg),
        };
        if descending { ord.reverse() } else { ord }
    });
}

fn cmp_f64(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

fn price_table(ui: &mut egui::Ui, metrics: &[AnalyticsPriceMetric], lang: Lang) {
    egui::Grid::new("analytics_price_metrics")
        .num_columns(6)
        .striped(true)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new(price_header_metric(lang)).weak());
            ui.label(egui::RichText::new(price_header_avg(lang)).weak())
                .on_hover_text(price_average_help(lang));
            ui.label(egui::RichText::new(price_header_weighted(lang)).weak())
                .on_hover_text(price_weighted_help(lang));
            ui.label(egui::RichText::new(price_header_median(lang)).weak())
                .on_hover_text(price_median_help(lang));
            ui.label(egui::RichText::new("P25\u{2013}P75").weak())
                .on_hover_text(price_range_help(lang));
            ui.label(egui::RichText::new(price_header_count(lang)).weak())
                .on_hover_text(price_count_help(lang));
            ui.end_row();
            for metric in metrics {
                if metric.count == 0 {
                    continue;
                }
                ui.label(price_metric_title(metric.kind, lang));
                ui.label(egui::RichText::new(fmt_decimal(metric.average, 3)).monospace());
                ui.label(egui::RichText::new(fmt_decimal(metric.weighted_average, 3)).monospace());
                ui.label(egui::RichText::new(fmt_decimal(metric.median, 3)).monospace());
                ui.label(
                    egui::RichText::new(format!(
                        "{} \u{2013} {}",
                        fmt_decimal(metric.p25, 3),
                        fmt_decimal(metric.p75, 3)
                    ))
                    .monospace(),
                );
                ui.label(egui::RichText::new(group_digits(metric.count)).monospace());
                ui.end_row();
            }
        });
}

fn pivot_dim_label(dim: PivotDim, lang: Lang) -> &'static str {
    let t = tr(lang);
    match dim {
        PivotDim::Recipient => t.recipient,
        PivotDim::Sender => t.sender,
        PivotDim::Edrpou => t.edrpou,
        PivotDim::ProductCode => t.product_code,
        PivotDim::Trademark => t.trademark,
        PivotDim::OriginCountry => t.origin_country,
        PivotDim::DispatchCountry => t.dispatch_country,
        PivotDim::TradeCountry => t.trade_country,
        PivotDim::Month => t.month,
        PivotDim::Year => t.year,
    }
}

const PIVOT_DIMS: [PivotDim; 10] = [
    PivotDim::Recipient,
    PivotDim::Sender,
    PivotDim::Edrpou,
    PivotDim::ProductCode,
    PivotDim::Trademark,
    PivotDim::OriginCountry,
    PivotDim::DispatchCountry,
    PivotDim::TradeCountry,
    PivotDim::Month,
    PivotDim::Year,
];

fn pivot_dim_combo(
    ui: &mut egui::Ui,
    id: &str,
    current: PivotDim,
    lang: Lang,
    out: &mut Option<PivotDim>,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(pivot_dim_label(current, lang))
        .show_ui(ui, |ui| {
            for dim in PIVOT_DIMS {
                if ui
                    .selectable_label(dim == current, pivot_dim_label(dim, lang))
                    .clicked()
                    && dim != current
                {
                    *out = Some(dim);
                }
            }
        });
}

/// Heatmap-style cross-tab. Row/column labels are clickable to drill into
/// the Results table; cell shading shows relative size within the matrix.
fn pivot_table_ui(
    ui: &mut egui::Ui,
    pivot: &PivotResult,
    row_dim: PivotDim,
    col_dim: PivotDim,
    metric: PivotMetric,
    lang: Lang,
) -> Option<AnalyticsFilterAction> {
    let mut action: Option<AnalyticsFilterAction> = None;
    let max_cell = pivot
        .cells
        .iter()
        .flat_map(|r| r.iter())
        .copied()
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let accent = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(80, 140, 255)
    } else {
        ACCENT
    };
    let total_label = tr(lang).total;

    egui::ScrollArea::both().show(ui, |ui| {
        let mut builder = TableBuilder::new(ui)
            .striped(false)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(190.0).at_least(120.0).clip(true));
        for _ in &pivot.col_labels {
            builder = builder.column(Column::initial(84.0).at_least(56.0));
        }
        builder = builder.column(Column::initial(92.0).at_least(64.0));
        builder
            .header(24.0, |mut header| {
                header.col(|ui| {
                    ui.strong(pivot_dim_label(row_dim, lang));
                });
                for (ci, label) in pivot.col_labels.iter().enumerate() {
                    header.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let is_others =
                                pivot.cols_truncated && ci + 1 == pivot.col_labels.len();
                            let label_text = egui::RichText::new(label.clone()).strong();
                            if is_others {
                                ui.label(label_text);
                            } else if let Some(next) = pivot_filter_action(col_dim, label.clone()) {
                                let response = ui
                                    .add(egui::Label::new(label_text).sense(egui::Sense::click()))
                                    .on_hover_text(pivot_click_hint(lang));
                                if response.clicked() {
                                    action = Some(next);
                                }
                            } else {
                                ui.label(label_text);
                            }
                        });
                    });
                }
                header.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.strong(total_label);
                    });
                });
            })
            .body(|mut body| {
                for (ri, row_label) in pivot.row_labels.iter().enumerate() {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            let resp = ui.add(
                                egui::Label::new(row_label)
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            let is_others =
                                pivot.rows_truncated && ri + 1 == pivot.row_labels.len();
                            if !is_others
                                && let Some(next) = pivot_filter_action(row_dim, row_label.clone())
                            {
                                let resp = resp.on_hover_text(pivot_click_hint(lang));
                                if resp.clicked() {
                                    action = Some(next);
                                }
                            }
                        });
                        for ci in 0..pivot.col_labels.len() {
                            let v = pivot.cells[ri][ci];
                            row.col(|ui| {
                                paint_pivot_cell(ui, v, max_cell, accent, metric);
                            });
                        }
                        row.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(pivot_fmt(
                                            pivot.row_totals[ri],
                                            metric,
                                        ))
                                        .monospace()
                                        .strong(),
                                    );
                                },
                            );
                        });
                    });
                }
                // Totals row.
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        ui.strong(total_label);
                    });
                    for ci in 0..pivot.col_labels.len() {
                        row.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(pivot_fmt(
                                            pivot.col_totals[ci],
                                            metric,
                                        ))
                                        .monospace()
                                        .strong(),
                                    );
                                },
                            );
                        });
                    }
                    row.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(pivot_fmt(pivot.grand_total, metric))
                                    .monospace()
                                    .strong(),
                            );
                        });
                    });
                });
            });
    });
    action
}

fn paint_pivot_cell(
    ui: &mut egui::Ui,
    value: f64,
    max_cell: f64,
    accent: egui::Color32,
    metric: PivotMetric,
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
    if value > 0.0 {
        let intensity = (value / max_cell).clamp(0.0, 1.0) as f32;
        // Stronger fill for larger cells (heatmap).
        let alpha = (18.0 + intensity * 150.0) as u8;
        let fill = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), alpha);
        ui.painter()
            .rect_filled(rect.shrink(1.0), egui::CornerRadius::same(2), fill);
        let text_color = ui.visuals().text_color();
        ui.painter().text(
            egui::pos2(rect.right() - 4.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            pivot_fmt(value, metric),
            egui::FontId::new(11.0, egui::FontFamily::Monospace),
            text_color,
        );
    }
}

fn pivot_fmt(value: f64, metric: PivotMetric) -> String {
    match metric {
        PivotMetric::Rows => group_digits(value as u64),
        _ => fmt_compact(value),
    }
}

fn pivot_click_hint(lang: Lang) -> &'static str {
    tr(lang).pivot_click_hint
}

/// Pivot matrix as TSV, ready to paste into Excel.
fn pivot_tsv(pivot: &PivotResult, row_dim: PivotDim, _col_dim: PivotDim, lang: Lang) -> String {
    let total_label = tr(lang).total;
    let mut out = String::new();
    out.push_str(pivot_dim_label(row_dim, lang));
    for c in &pivot.col_labels {
        out.push('\t');
        out.push_str(c);
    }
    out.push('\t');
    out.push_str(total_label);
    for (ri, rl) in pivot.row_labels.iter().enumerate() {
        out.push('\n');
        out.push_str(rl);
        for ci in 0..pivot.col_labels.len() {
            out.push('\t');
            out.push_str(&format!("{:.2}", pivot.cells[ri][ci]));
        }
        out.push('\t');
        out.push_str(&format!("{:.2}", pivot.row_totals[ri]));
    }
    out.push('\n');
    out.push_str(total_label);
    for ci in 0..pivot.col_labels.len() {
        out.push('\t');
        out.push_str(&format!("{:.2}", pivot.col_totals[ci]));
    }
    out.push('\t');
    out.push_str(&format!("{:.2}", pivot.grand_total));
    out
}

/// Table of flagged undervalued declarations. Returns a record id when a row
/// is clicked (to open its card). `rescan` is set if the user asks to refresh.
fn underpricing_table(
    ui: &mut egui::Ui,
    uv: &Undervaluation,
    lang: Lang,
    rescan: &mut bool,
) -> Option<i64> {
    let mut open_id = None;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(fmt(
                tr(lang).underpricing_found,
                &[
                    &group_digits(uv.rows.len() as u64),
                    &group_digits(uv.checked_codes),
                ],
            ))
            .weak()
            .small(),
        );
        if ui.small_button(tr(lang).underpricing_rescan).clicked() {
            *rescan = true;
        }
    });
    if uv.rows.is_empty() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(tr(lang).underpricing_none).weak());
        return None;
    }
    ui.add_space(4.0);
    let (date_h, recip_h, code_h, desc_h) = (
        tr(lang).year,
        tr(lang).recipient,
        tr(lang).product_code,
        tr(lang).description,
    );
    let price_h = tr(lang).per_kg;
    let median_h = tr(lang).median;
    let below_h = tr(lang).below_by;
    let _ = date_h;
    egui::ScrollArea::horizontal()
        .id_salt("underpricing_scroll")
        .show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(82.0).at_least(70.0))
                .column(Column::initial(180.0).at_least(100.0).clip(true))
                .column(Column::initial(96.0).at_least(70.0))
                .column(Column::initial(300.0).at_least(120.0).clip(true))
                .column(Column::initial(70.0).at_least(50.0))
                .column(Column::initial(70.0).at_least(50.0))
                .column(Column::initial(72.0).at_least(56.0))
                .header(24.0, |mut h| {
                    h.col(|ui| {
                        ui.strong(header_for("declaration_date"));
                    });
                    h.col(|ui| {
                        ui.strong(recip_h);
                    });
                    h.col(|ui| {
                        ui.strong(code_h);
                    });
                    h.col(|ui| {
                        ui.strong(desc_h);
                    });
                    h.col(|ui| {
                        ui.strong(price_h);
                    });
                    h.col(|ui| {
                        ui.strong(median_h);
                    });
                    h.col(|ui| {
                        ui.strong(below_h);
                    });
                })
                .body(|mut body| {
                    for row in &uv.rows {
                        body.row(22.0, |mut tr_row| {
                            let mut clicked = false;
                            tr_row.col(|ui| {
                                clicked |= ui
                                    .add(egui::Label::new(&row.declaration_date).selectable(false))
                                    .clicked();
                            });
                            tr_row.col(|ui| {
                                clicked |= ui
                                    .add(
                                        egui::Label::new(&row.recipient)
                                            .selectable(false)
                                            .truncate(),
                                    )
                                    .clicked();
                            });
                            tr_row.col(|ui| {
                                clicked |= ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(&row.product_code).monospace(),
                                        )
                                        .selectable(false),
                                    )
                                    .clicked();
                            });
                            tr_row.col(|ui| {
                                clicked |= ui
                                    .add(
                                        egui::Label::new(&row.description)
                                            .selectable(false)
                                            .truncate(),
                                    )
                                    .clicked();
                            });
                            tr_row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(fmt_decimal(row.price_per_kg, 2))
                                        .monospace()
                                        .color(egui::Color32::from_rgb(200, 60, 60)),
                                );
                            });
                            tr_row.col(|ui| {
                                ui.label(
                                    egui::RichText::new(fmt_decimal(row.code_median, 2))
                                        .monospace(),
                                );
                            });
                            tr_row.col(|ui| {
                                let pct = ((1.0 - row.ratio) * 100.0).round() as i64;
                                ui.label(
                                    egui::RichText::new(format!("{pct}%")).monospace().strong(),
                                );
                            });
                            if clicked {
                                open_id = Some(row.id);
                            }
                        });
                    }
                });
        });
    open_id
}

fn price_header_median(lang: Lang) -> &'static str {
    tr(lang).median
}

fn price_header_weighted(lang: Lang) -> &'static str {
    tr(lang).weighted_avg
}

fn top_share_pattern(lang: Lang) -> &'static str {
    tr(lang).top_share_pattern
}

fn group_explorer_title(kind: AnalyticsSectionKind, lang: Lang) -> String {
    fmt(tr(lang).group_all_title, &[section_title(kind, lang)])
}

fn group_explorer_hint(lang: Lang) -> &'static str {
    tr(lang).group_explorer_hint
}

fn group_search_hint(lang: Lang) -> &'static str {
    tr(lang).group_search_hint
}

fn group_explorer_count(rows: u64, limited: bool, lang: Lang) -> String {
    let pattern = if limited {
        tr(lang).group_loaded_first
    } else {
        tr(lang).group_loaded_rows
    };
    fmt(pattern, &[&group_digits(rows)])
}

fn group_visible_count(visible: u64, total: u64, limited: bool, lang: Lang) -> String {
    let pattern = if limited {
        tr(lang).group_showing_first
    } else {
        tr(lang).group_showing
    };
    fmt(pattern, &[&group_digits(visible), &group_digits(total)])
}

fn copy_visible_label(lang: Lang) -> &'static str {
    tr(lang).copy_visible
}

fn label_header(lang: Lang) -> &'static str {
    tr(lang).col_label
}

fn companies_header(lang: Lang) -> &'static str {
    tr(lang).col_companies
}

fn share_header(lang: Lang) -> &'static str {
    tr(lang).col_share
}

fn section_title(kind: AnalyticsSectionKind, lang: Lang) -> &'static str {
    let t = tr(lang);
    match kind {
        AnalyticsSectionKind::Recipients => t.sec_recipients,
        AnalyticsSectionKind::Senders => t.sec_senders,
        AnalyticsSectionKind::Edrpou => t.sec_edrpou,
        AnalyticsSectionKind::ProductCodes => t.sec_product_codes,
        AnalyticsSectionKind::Trademarks => t.sec_trademarks,
        AnalyticsSectionKind::ProductGroups => t.sec_product_groups,
        AnalyticsSectionKind::OriginCountries => t.sec_origin_countries,
        AnalyticsSectionKind::DispatchCountries => t.sec_dispatch_countries,
        AnalyticsSectionKind::TradeCountries => t.sec_trade_countries,
    }
}

fn row_counts_label(row: &AnalyticsGroupRow, lang: Lang) -> String {
    fmt(
        tr(lang).row_counts,
        &[
            &group_digits(row.rows),
            &group_digits(row.declarations),
            &group_digits(row.companies),
        ],
    )
}

fn row_hover_text(row: &AnalyticsGroupRow, lang: Lang) -> String {
    let counts = row_counts_label(row, lang);
    fmt(
        tr(lang).row_hover,
        &[
            &row.label,
            &counts,
            &fmt_decimal(row.total_value_usd, 2),
            &fmt_decimal(row.total_net_kg, 3),
            &fmt_decimal(row.share_percent, 2),
            &fmt_decimal(row.avg_value_per_net_kg, 2),
        ],
    )
}

fn price_metric_title(kind: PriceMetricKind, lang: Lang) -> &'static str {
    let t = tr(lang);
    match kind {
        PriceMetricKind::ValuePerNetKg => t.pm_value_per_net_kg,
        PriceMetricKind::RfvUsdKg => t.pm_rfv,
        PriceMetricKind::RmvNetUsdKg => t.pm_rmv_net,
        PriceMetricKind::RmvUsdExtraUnit => t.pm_rmv_extra_unit,
        PriceMetricKind::RmvGrossUsdKg => t.pm_rmv_gross,
        PriceMetricKind::MinBaseUsdKg => t.pm_min_base,
    }
}

fn price_header_metric(lang: Lang) -> &'static str {
    tr(lang).price_header_metric
}

fn price_header_avg(lang: Lang) -> &'static str {
    tr(lang).price_header_avg
}

fn price_header_count(lang: Lang) -> &'static str {
    tr(lang).price_header_count
}

fn fmt_decimal(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let mut s = format!("{value:.decimals$}");
    if let Some(dot) = s.find('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.len() == dot + 1 {
            s.pop();
        }
    }
    let (sign, body) = s
        .strip_prefix('-')
        .map(|rest| ("-", rest))
        .unwrap_or(("", s.as_str()));
    let (int_part, frac_part) = body.split_once('.').unwrap_or((body, ""));
    let mut grouped = String::with_capacity(s.len() + s.len() / 3);
    grouped.push_str(sign);
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i).is_multiple_of(3) {
            grouped.push('\u{202F}');
        }
        grouped.push(ch);
    }
    if !frac_part.is_empty() {
        grouped.push('.');
        grouped.push_str(frac_part);
    }
    grouped
}

fn result_col_index(name: &str) -> Option<usize> {
    RESULT_COLUMNS.iter().position(|column| *column == name)
}

fn filter_field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32, search: &mut bool) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().weak());
        let response = ui.add(egui::TextEdit::singleline(value).desired_width(width));
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            *search = true;
        }
    });
}

/// System font candidates per OS. The first readable file wins; when none
/// is found, egui's bundled fonts are used (they cover Cyrillic too).
fn system_font_candidates() -> (&'static [&'static str], &'static [&'static str]) {
    #[cfg(target_os = "windows")]
    {
        (
            &["C:\\Windows\\Fonts\\segoeui.ttf"],
            &["C:\\Windows\\Fonts\\consola.ttf"],
        )
    }
    #[cfg(target_os = "macos")]
    {
        // Single-file .ttf fonts that cover Cyrillic. If none is found, egui's
        // bundled font still renders Cyrillic, so text is never broken.
        (
            &[
                "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                "/System/Library/Fonts/Supplemental/Arial.ttf",
                "/System/Library/Fonts/Supplemental/Verdana.ttf",
                "/System/Library/Fonts/Supplemental/Tahoma.ttf",
                "/Library/Fonts/Arial Unicode.ttf",
                "/Library/Fonts/Arial.ttf",
            ],
            &[
                "/System/Library/Fonts/Supplemental/Courier New.ttf",
                "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
                "/Library/Fonts/Courier New.ttf",
            ],
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        (
            &[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            ],
            &[
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
                "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            ],
        )
    }
}

fn load_first_font(
    fonts: &mut egui::FontDefinitions,
    family: egui::FontFamily,
    key: &str,
    candidates: &[&str],
) {
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(key.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, key.to_owned());
            return;
        }
    }
}

/// CJK-capable system fonts per OS, tried in order. Used only as a fallback so
/// the Chinese interface renders; these ship by default on Windows and macOS,
/// and come from the Noto/WenQuanYi packages on Linux.
fn cjk_font_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &[
            "C:\\Windows\\Fonts\\msyh.ttc",
            "C:\\Windows\\Fonts\\msyh.ttf",
            "C:\\Windows\\Fonts\\simsun.ttc",
            "C:\\Windows\\Fonts\\simhei.ttf",
        ]
    }
    #[cfg(target_os = "macos")]
    {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-zenhei/wqy-zenhei.ttc",
        ]
    }
}

/// Inserts the first available CJK font as a fallback at the end of both font
/// families. Missing on a machine only affects Chinese; all other text uses the
/// primary fonts as before.
fn load_cjk_fallback(fonts: &mut egui::FontDefinitions, candidates: &[&str]) {
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let key = "cjk-fallback".to_owned();
            fonts
                .font_data
                .insert(key.clone(), Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push(key.clone());
            }
            return;
        }
    }
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let (proportional, monospace) = system_font_candidates();
    // Native system font with complete Cyrillic coverage when available.
    load_first_font(
        &mut fonts,
        egui::FontFamily::Proportional,
        "system-ui",
        proportional,
    );
    // System monospace for codes and numbers.
    load_first_font(
        &mut fonts,
        egui::FontFamily::Monospace,
        "system-mono",
        monospace,
    );
    // CJK fallback so the Chinese interface renders without bundling a large
    // font. Appended after the primary families, so it is used only for glyphs
    // the primary font lacks (Latin/Cyrillic stay on the system font).
    load_cjk_fallback(&mut fonts, cjk_font_candidates());
    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.5, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(14.5, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(19.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(13.5, FontFamily::Monospace),
        );
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        style.visuals.hyperlink_color = ACCENT;
        style.visuals.slider_trailing_fill = true;
    });
    // Table striping with more contrast than the default.
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals.faint_bg_color = egui::Color32::from_gray(34);
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.visuals.faint_bg_color = egui::Color32::from_gray(244);
    });
}

#[cfg(test)]
mod tests {
    use super::{
        GuidedQuestionKind, StoredQuery, decode_stored_queries, encode_stored_queries,
        exact_edrpou_candidate, guided_question_title, guided_questions_for,
        invalidate_underpricing_generation,
    };
    use crate::db::{Filters, Query};
    use crate::i18n::Lang;

    #[test]
    fn invalidating_underpricing_generation_rejects_stale_results() {
        let mut generation = 7;
        let stale_generation = generation;

        invalidate_underpricing_generation(&mut generation);

        assert_ne!(generation, stale_generation);
    }

    #[test]
    fn stored_queries_round_trip_full_query() {
        let query = Query {
            text: "Apple\tphones%2024".into(),
            filters: Filters {
                year: "2024".into(),
                product_code: "8517".into(),
                sender: "A\nB".into(),
                origin_country: "CN".into(),
                ..Filters::default()
            },
        };
        let stored = vec![StoredQuery {
            name: "Apple saved".into(),
            query: query.clone(),
        }];

        let encoded = encode_stored_queries(&stored);
        let decoded = decode_stored_queries(&encoded);

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].name, "Apple saved");
        assert_eq!(decoded[0].query, query);
    }

    #[test]
    fn guided_questions_cover_all_languages() {
        let kinds = [
            GuidedQuestionKind::ProductCompanies,
            GuidedQuestionKind::ProductAllCompanies,
            GuidedQuestionKind::ProductGoods,
            GuidedQuestionKind::ProductCountries,
            GuidedQuestionKind::ProductPrices,
            GuidedQuestionKind::ProductTimeline,
            GuidedQuestionKind::ProductCompaniesByMonth,
            GuidedQuestionKind::CompanyProfile,
            GuidedQuestionKind::CompanyGoods,
            GuidedQuestionKind::CompanySuppliers,
            GuidedQuestionKind::CompanyCountries,
            GuidedQuestionKind::CompanyTimeline,
            GuidedQuestionKind::CompanyGoodsByMonth,
            GuidedQuestionKind::MarketCompanies,
            GuidedQuestionKind::MarketGoods,
            GuidedQuestionKind::MarketCountries,
            GuidedQuestionKind::MarketPrices,
        ];
        for lang in Lang::ALL {
            for kind in kinds {
                assert!(!guided_question_title(kind, lang).trim().is_empty());
            }
        }
    }

    #[test]
    fn guided_questions_match_input_context() {
        let product = guided_questions_for("Apple", &Filters::default());
        assert!(
            product
                .iter()
                .any(|(_, kind)| *kind == GuidedQuestionKind::ProductCompanies)
        );
        assert!(
            product
                .iter()
                .any(|(_, kind)| *kind == GuidedQuestionKind::ProductPrices)
        );

        let filters = Filters {
            edrpou: "12345678".into(),
            ..Filters::default()
        };
        let company = guided_questions_for("", &filters);
        assert!(
            company
                .iter()
                .any(|(_, kind)| *kind == GuidedQuestionKind::CompanyProfile)
        );
        assert_eq!(
            exact_edrpou_candidate("", &filters),
            Some("12345678".to_string())
        );

        let filters = Filters {
            year: "2024".into(),
            origin_country: "CN".into(),
            ..Filters::default()
        };
        let market = guided_questions_for("", &filters);
        assert!(
            market
                .iter()
                .any(|(_, kind)| *kind == GuidedQuestionKind::MarketCompanies)
        );
        assert!(
            market
                .iter()
                .any(|(_, kind)| *kind == GuidedQuestionKind::MarketPrices)
        );
    }
}
