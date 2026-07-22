use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Duration, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

const RESET_RAW_MAX_CHARS: usize = 64;

pub(crate) fn minutes_until(
    raw: &str,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<i64> {
    let epoch = reset_epoch(raw, now_epoch, reset_description_offset_minutes)?;
    Some(epoch.checked_sub(now_epoch)?.max(0) / 60)
}

pub(crate) fn reset_epoch(
    raw: &str,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "null" || raw.chars().count() > RESET_RAW_MAX_CHARS {
        return None;
    }
    if let Some(epoch) = parse_offset_datetime(raw) {
        return Some(epoch);
    }

    let desc = raw
        .strip_prefix("Resets ")
        .or_else(|| raw.strip_prefix("resets "))?;

    parse_description_epoch(desc, now_epoch, reset_description_offset_minutes)
}

pub(crate) fn parse_offset_datetime(raw: &str) -> Option<i64> {
    if let Ok(parsed) = OffsetDateTime::parse(raw, &Rfc3339) {
        return Some(parsed.unix_timestamp());
    }
    let normalized = normalize_colonless_offset(raw)?;
    OffsetDateTime::parse(&normalized, &Rfc3339)
        .ok()
        .map(|parsed| parsed.unix_timestamp())
}

fn normalize_colonless_offset(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    if bytes.len() < 5 {
        return None;
    }
    let sign = bytes.len() - 5;
    if !matches!(bytes[sign], b'+' | b'-') {
        return None;
    }
    if !bytes[sign + 1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut normalized = String::with_capacity(raw.len() + 1);
    normalized.push_str(&raw[..raw.len() - 2]);
    normalized.push(':');
    normalized.push_str(&raw[raw.len() - 2..]);
    Some(normalized)
}

fn local_offset_at(datetime: OffsetDateTime) -> UtcOffset {
    UtcOffset::local_offset_at(datetime)
        .or_else(|_| UtcOffset::current_local_offset())
        .unwrap_or(UtcOffset::UTC)
}

fn configured_reset_description_offset(offset_minutes: Option<i16>) -> Option<UtcOffset> {
    let seconds = i32::from(offset_minutes?).checked_mul(60)?;
    UtcOffset::from_whole_seconds(seconds).ok()
}

pub(crate) fn assume_local(
    local: PrimitiveDateTime,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<OffsetDateTime> {
    if let Some(offset) = configured_reset_description_offset(reset_description_offset_minutes) {
        return Some(local.assume_offset(offset));
    }
    let now = OffsetDateTime::from_unix_timestamp(now_epoch).ok()?;
    let mut offset = local_offset_at(now);
    for _ in 0..2 {
        let candidate = local.assume_offset(offset);
        let next = local_offset_at(candidate);
        if next == offset {
            return Some(candidate);
        }
        offset = next;
    }
    Some(local.assume_offset(offset))
}

pub(crate) fn parse_description_epoch(
    desc: &str,
    now_epoch: i64,
    reset_description_offset_minutes: Option<i16>,
) -> Option<i64> {
    let now = OffsetDateTime::from_unix_timestamp(now_epoch).ok()?;
    let local_offset = configured_reset_description_offset(reset_description_offset_minutes)
        .unwrap_or_else(|| local_offset_at(now));
    let short = format_description!(
        "[month repr:short] [day padding:none], [year] [hour repr:12 padding:none]:[minute] [period case:upper]"
    );
    let long = format_description!(
        "[month repr:long] [day padding:none], [year] [hour repr:12 padding:none]:[minute] [period case:upper]"
    );
    if let Ok(parsed) =
        PrimitiveDateTime::parse(desc, &short).or_else(|_| PrimitiveDateTime::parse(desc, &long))
    {
        return assume_local(parsed, now_epoch, reset_description_offset_minutes)
            .map(|parsed| parsed.unix_timestamp());
    }

    let clock = parse_time_12h(desc)?;
    let today = now.to_offset(local_offset).date();
    let mut local = PrimitiveDateTime::new(today, clock);
    let mut epoch =
        assume_local(local, now_epoch, reset_description_offset_minutes)?.unix_timestamp();
    if epoch < now_epoch {
        local = local.checked_add(Duration::days(1))?;
        epoch = assume_local(local, now_epoch, reset_description_offset_minutes)?.unix_timestamp();
    }
    Some(epoch)
}

fn parse_time_12h(desc: &str) -> Option<Time> {
    let (time_part, period) = desc.rsplit_once(' ')?;
    let (hour, minute) = time_part.split_once(':')?;
    let mut hour: u8 = hour.parse().ok()?;
    let minute: u8 = minute.parse().ok()?;
    if !(1..=12).contains(&hour) || minute > 59 {
        return None;
    }
    match period {
        "AM" => {
            if hour == 12 {
                hour = 0;
            }
        }
        "PM" => {
            if hour != 12 {
                hour += 12;
            }
        }
        _ => return None,
    }
    Time::from_hms(hour, minute, 0).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_overlong_reset_strings() {
        let raw = format!("Resets {}", "1".repeat(65));
        assert_eq!(reset_epoch(&raw, 0, Some(0)), None);
    }

    #[test]
    fn overflowed_countdown_is_unknown() {
        assert_eq!(
            minutes_until("1970-01-01T00:00:00Z", i64::MIN, Some(0)),
            None
        );
    }
}
