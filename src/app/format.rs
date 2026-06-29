/// "2024-03" -> "03'24".
pub(super) fn short_month(month: &str) -> String {
    match (month.get(0..4), month.get(5..7)) {
        (Some(year), Some(m)) => format!("{m}'{}", &year[2..]),
        _ => month.to_string(),
    }
}

/// Compact number for chart captions: 12.4M, 980K, 312.
pub(super) fn fmt_compact(value: f64) -> String {
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

pub(super) fn fmt_decimal(value: f64, decimals: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::{fmt_compact, fmt_decimal, short_month};

    #[test]
    fn short_month_compacts_iso_month() {
        assert_eq!(short_month("2024-03"), "03'24");
        assert_eq!(short_month("bad"), "bad");
    }

    #[test]
    fn compact_numbers_match_existing_ui_scale() {
        assert_eq!(fmt_compact(12_400_000.0), "12.4M");
        assert_eq!(fmt_compact(980_000.0), "980K");
        assert_eq!(fmt_compact(312.0), "312");
        assert_eq!(fmt_compact(9.25), "9.2");
    }

    #[test]
    fn decimals_trim_zeroes_and_group_thousands() {
        assert_eq!(fmt_decimal(1234.50, 2), "1\u{202F}234.5");
        assert_eq!(fmt_decimal(-1234567.0, 2), "-1\u{202F}234\u{202F}567");
        assert_eq!(fmt_decimal(f64::NAN, 2), "0");
    }
}
