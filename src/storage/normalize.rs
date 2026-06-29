pub fn parse_number(value: &str) -> Option<f64> {
    let mut compact = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_digit() || matches!(ch, '.' | ',' | '-' | '+') {
            compact.push(ch);
        }
    }
    if !compact.chars().any(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let dot_count = compact.matches('.').count();
    let comma_count = compact.matches(',').count();
    let decimal_sep = match (dot_count, comma_count) {
        (0, 0) => None,
        (0, 1) => decimal_separator_for_single(&compact, ','),
        (1, 0) => decimal_separator_for_single(&compact, '.'),
        (0, _) | (_, 0) => None,
        _ => {
            let last_dot = compact.rfind('.').unwrap_or(0);
            let last_comma = compact.rfind(',').unwrap_or(0);
            Some(if last_dot > last_comma { '.' } else { ',' })
        }
    };

    let mut normalized = String::with_capacity(compact.len());
    let mut sign_written = false;
    let mut decimal_written = false;
    for (i, ch) in compact.chars().enumerate() {
        if ch.is_ascii_digit() {
            normalized.push(ch);
        } else if matches!(ch, '-' | '+') && !sign_written && normalized.is_empty() && i == 0 {
            normalized.push(ch);
            sign_written = true;
        } else if Some(ch) == decimal_sep && !decimal_written {
            normalized.push('.');
            decimal_written = true;
        }
    }

    normalized.parse::<f64>().ok()
}

pub(crate) fn parse_year(value: &str) -> Option<i64> {
    let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 4 {
        digits.parse().ok()
    } else {
        None
    }
}

pub(crate) fn month_key(value: &str) -> String {
    let trimmed = value.trim();
    let date = trimmed.split([' ', 'T']).next().unwrap_or(trimmed);
    let parts: Vec<&str> = date.split(['.', '/', '-']).collect();
    if parts.len() == 3 {
        if parts[0].len() <= 2
            && parts[1].len() <= 2
            && parts[2].len() == 4
            && let (Ok(_d), Ok(m), Ok(y)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<u32>(),
            )
            && (1..=12).contains(&m)
        {
            return format!("{y:04}-{m:02}");
        }
        if parts[0].len() <= 2
            && parts[1].len() <= 2
            && parts[2].len() == 4
            && let (Ok(m), Ok(_d), Ok(y)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<u32>(),
            )
            && (1..=12).contains(&m)
        {
            return format!("{y:04}-{m:02}");
        }
        if parts[0].len() == 4
            && parts[1].len() <= 2
            && parts[2].len() <= 2
            && let (Ok(y), Ok(m), Ok(_d)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<u32>(),
            )
            && (1..=12).contains(&m)
        {
            return format!("{y:04}-{m:02}");
        }
    }
    String::new()
}

pub(crate) fn normalize_country_key(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let key: String = trimmed
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect();
    if matches!(
        key.as_str(),
        "0" | "00" | "000" | "NA" | "NODATA" | "ND" | "НД" | "НЕМАДАНИХ" | "НЕТДАННЫХ"
    ) {
        return String::new();
    }
    match key.as_str() {
        "CN" | "CHN" | "CHINA" | "КИТАЙ" => "CN",
        "IE" | "IRL" | "IRELAND" | "ІРЛАНДІЯ" | "ИРЛАНДИЯ" => "IE",
        "PL" | "POL" | "POLAND" | "ПОЛЬЩА" | "ПОЛЬША" => "PL",
        "CZ"
        | "CZE"
        | "CZECHIA"
        | "CZECHREPUBLIC"
        | "ЧЕСЬКАРЕСПУБЛІКА"
        | "ЧЕХІЯ"
        | "ЧЕШСКАЯРЕСПУБЛИКА"
        | "ЧЕХИЯ" => "CZ",
        "DE" | "DEU" | "GERMANY" | "НІМЕЧЧИНА" | "ГЕРМАНІЯ" | "ГЕРМАНИЯ" => {
            "DE"
        }
        "US"
        | "USA"
        | "UNITEDSTATES"
        | "UNITEDSTATESOFAMERICA"
        | "СПОЛУЧЕНІШТАТИАМЕРИКИ"
        | "США"
        | "СОЕДИНЕННЫЕШТАТЫАМЕРИКИ" => "US",
        "VN" | "VNM" | "VIETNAM" | "ВЄТНАМ" | "ВЕТНАМ" => "VN",
        "EU" | "EUROPEANUNION" | "КРАЇНИЄС" | "СТРАНЫЕС" => "EU",
        "KR"
        | "KOR"
        | "SOUTHKOREA"
        | "REPUBLICOFKOREA"
        | "ПІВДЕННАКОРЕЯ"
        | "КОРЕЯРЕСПУБЛІКА"
        | "ЮЖНАЯКОРЕЯ" => "KR",
        "TR" | "TUR" | "TURKEY" | "TURKIYE" | "ТУРЕЧЧИНА" | "ТУРЦІЯ" | "ТУРЦИЯ" => {
            "TR"
        }
        "IN" | "IND" | "INDIA" | "ІНДІЯ" | "ИНДИЯ" => "IN",
        "IT" | "ITA" | "ITALY" | "ІТАЛІЯ" | "ИТАЛИЯ" => "IT",
        "BE" | "BEL" | "BELGIUM" | "БЕЛЬГІЯ" | "БЕЛЬГИЯ" => "BE",
        "NL" | "NLD" | "NETHERLANDS" | "НІДЕРЛАНДИ" | "НИДЕРЛАНДЫ" => "NL",
        "FR" | "FRA" | "FRANCE" | "ФРАНЦІЯ" | "ФРАНЦИЯ" => "FR",
        "GB"
        | "UK"
        | "GBR"
        | "GREATBRITAIN"
        | "UNITEDKINGDOM"
        | "ВЕЛИКАБРИТАНІЯ"
        | "ВЕЛИКОБРИТАНІЯ"
        | "ВЕЛИКОБРИТАНИЯ" => "GB",
        "ES" | "ESP" | "SPAIN" | "ІСПАНІЯ" | "ИСПАНИЯ" => "ES",
        "CH" | "CHE" | "SWITZERLAND" | "ШВЕЙЦАРІЯ" | "ШВЕЙЦАРИЯ" => "CH",
        "AT" | "AUT" | "AUSTRIA" | "АВСТРІЯ" | "АВСТРИЯ" => "AT",
        "FI" | "FIN" | "FINLAND" | "ФІНЛЯНДІЯ" | "ФИНЛЯНДИЯ" => "FI",
        "LV" | "LVA" | "LATVIA" | "ЛАТВІЯ" | "ЛАТВИЯ" => "LV",
        "LT" | "LTU" | "LITHUANIA" | "ЛИТВА" => "LT",
        "EE" | "EST" | "ESTONIA" | "ЕСТОНІЯ" | "ЭСТОНИЯ" => "EE",
        "HU" | "HUN" | "HUNGARY" | "УГОРЩИНА" | "ВЕНГРИЯ" => "HU",
        "RO" | "ROU" | "ROMANIA" | "РУМУНІЯ" | "РУМЫНИЯ" => "RO",
        "BG" | "BGR" | "BULGARIA" | "БОЛГАРІЯ" | "БОЛГАРИЯ" => "BG",
        _ => return key,
    }
    .to_string()
}

pub(crate) fn normalize_text_key(value: &str) -> String {
    let mut out = String::new();
    let mut last_space = false;
    for ch in value.trim().chars() {
        if ch.is_whitespace() {
            if !out.is_empty() && !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.extend(ch.to_lowercase());
            last_space = false;
        }
    }
    out
}

pub(crate) fn clean_label_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let key: String = trimmed
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect();
    if matches!(
        key.as_str(),
        "0" | "00"
            | "000"
            | "0000"
            | "NA"
            | "NА"
            | "ND"
            | "NULL"
            | "NONE"
            | "NODATA"
            | "UNKNOWN"
            | "НД"
            | "НЕМАДАНИХ"
            | "НЕТДАННЫХ"
            | "НЕВІДОМО"
            | "НЕИЗВЕСТНО"
    ) {
        String::new()
    } else {
        trimmed.to_string()
    }
}

/// Extracts a 20xx year from date text.
pub fn extract_year(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    for window_start in 0..bytes.len().saturating_sub(3) {
        let w = &bytes[window_start..window_start + 4];
        if w[0] == b'2' && w[1] == b'0' && w[2].is_ascii_digit() && w[3].is_ascii_digit() {
            let before_digit = window_start > 0 && bytes[window_start - 1].is_ascii_digit();
            let after_digit =
                window_start + 4 < bytes.len() && bytes[window_start + 4].is_ascii_digit();
            if !before_digit && !after_digit {
                return std::str::from_utf8(w).ok()?.parse().ok();
            }
        }
    }
    None
}

fn decimal_separator_for_single(value: &str, sep: char) -> Option<char> {
    let pos = value.rfind(sep)?;
    let after = value[pos + sep.len_utf8()..]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .count();
    if after == 0 { None } else { Some(sep) }
}
