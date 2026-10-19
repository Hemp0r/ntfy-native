use crate::i18n::{UiStrings, fill};
use chrono::{DateTime, Local, NaiveTime, TimeZone};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseChoice {
    Minutes15,
    Hour1,
    UntilTomorrow,
    Indefinitely,
    Resume,
}

pub fn pause_until<Tz: TimeZone>(choice: PauseChoice, now: DateTime<Tz>) -> Option<i64> {
    let ts = now.timestamp();
    match choice {
        PauseChoice::Minutes15 => Some(ts.saturating_add(15 * 60)),
        PauseChoice::Hour1 => Some(ts.saturating_add(3600)),
        PauseChoice::Indefinitely => Some(i64::MAX),
        PauseChoice::Resume => None,
        PauseChoice::UntilTomorrow => {
            let morning = NaiveTime::from_hms_opt(8, 0, 0)?;
            let today = now.date_naive();
            let date = if now.time() < morning {
                today
            } else {
                today.succ_opt()?
            };
            let mut local = date.and_time(morning);
            // Skip over a DST gap at 08:00 by moving forward to the first valid time.
            for _ in 0..4 {
                if let Some(dt) = now.timezone().from_local_datetime(&local).earliest() {
                    return Some(dt.timestamp());
                }
                local += chrono::Duration::hours(1);
            }
            None
        }
    }
}

pub fn format_age(now: i64, then: i64, s: &UiStrings) -> String {
    format_age_in(now, then, s, &Local)
}

pub fn format_age_in<Tz: TimeZone>(now: i64, then: i64, s: &UiStrings, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let age = now.saturating_sub(then);
    if age < 60 {
        s.just_now.to_string()
    } else if age < 3600 {
        fill(s.minutes_ago, &[("n", &(age / 60).to_string())])
    } else if age < 86400 {
        fill(s.hours_ago, &[("n", &(age / 3600).to_string())])
    } else {
        match tz.timestamp_opt(then, 0).single() {
            Some(dt) => dt.format("%d.%m. %H:%M").to_string(),
            None => String::new(),
        }
    }
}

pub fn format_paused(until: i64, now: i64, s: &UiStrings) -> String {
    format_paused_in(until, now, s, &Local)
}

pub fn format_paused_in<Tz: TimeZone>(until: i64, now: i64, s: &UiStrings, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    if until == i64::MAX {
        return s.paused.to_string();
    }
    let (Some(u), Some(n)) = (
        tz.timestamp_opt(until, 0).single(),
        tz.timestamp_opt(now, 0).single(),
    ) else {
        return s.paused.to_string();
    };
    let time = u.format("%H:%M").to_string();
    let template = if u.date_naive() == n.date_naive() {
        s.paused_until
    } else {
        s.paused_until_tomorrow
    };
    fill(template, &[("time", &time)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::UI_DE;
    use chrono::FixedOffset;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn pause_choices() {
        let now = at("2026-10-19T22:30:00+02:00");
        let t = now.timestamp();
        assert_eq!(pause_until(PauseChoice::Minutes15, now), Some(t + 900));
        assert_eq!(pause_until(PauseChoice::Hour1, now), Some(t + 3600));
        assert_eq!(
            pause_until(PauseChoice::UntilTomorrow, now),
            Some(at("2026-10-20T08:00:00+02:00").timestamp())
        );
        assert_eq!(pause_until(PauseChoice::Indefinitely, now), Some(i64::MAX));
        assert_eq!(pause_until(PauseChoice::Resume, now), None);
    }

    #[test]
    fn pause_tomorrow_after_midnight_is_same_day_morning() {
        let now = at("2026-10-20T01:00:00+02:00");
        assert_eq!(
            pause_until(PauseChoice::UntilTomorrow, now),
            Some(at("2026-10-20T08:00:00+02:00").timestamp())
        );
        let now = at("2026-10-20T08:00:00+02:00");
        assert_eq!(
            pause_until(PauseChoice::UntilTomorrow, now),
            Some(at("2026-10-21T08:00:00+02:00").timestamp())
        );
    }

    #[test]
    fn ages() {
        let now = at("2026-10-19T12:00:00+02:00").timestamp();
        let age = |d: i64| format_age_in(now, now - d, &UI_DE, &tz());
        assert_eq!(age(30), "gerade eben");
        assert_eq!(age(-5), "gerade eben");
        assert_eq!(age(300), "vor 5 Min.");
        assert_eq!(age(7200), "vor 2 Std.");
        assert_eq!(age(2 * 86400), "17.10. 12:00");
    }

    #[test]
    fn paused_labels() {
        let now = at("2026-10-19T10:00:00+02:00").timestamp();
        let p = |u: &str| format_paused_in(at(u).timestamp(), now, &UI_DE, &tz());
        assert_eq!(p("2026-10-19T11:15:00+02:00"), "Pausiert bis 11:15");
        assert_eq!(p("2026-10-20T08:00:00+02:00"), "Pausiert bis morgen 08:00");
        assert_eq!(format_paused_in(i64::MAX, now, &UI_DE, &tz()), "Pausiert");
    }
}
