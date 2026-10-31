//! Telling a usage limit (wait, then go on in the same conversation) from a
//! lack of credits (tell someone) from an ordinary failure — in the words the
//! CLIs actually print.

use chrono::{DateTime, Duration, Utc};

/// The account ran into its window: the work waits and resumes, and it
/// never counts as a failure.
const LIMIT: &[&str] = &[
    "usage limit reached",
    "usage limit",
    "rate limit",
    "rate_limit_error",
    "rate_limit_exceeded",
    "usage_limit_reached",
    "you've hit your usage limit",
    // Claude Code's current words: «You've hit your 5-hour limit · resets
    // 10:40pm (America/Sao_Paulo)», and the weekly one.
    "hit your 5-hour limit",
    "hit your weekly limit",
    "hit your session limit",
    "hit your limit",
    "please try again later",
    "overloaded_error",
];

/// Money, not time: nothing resets on its own.
const NO_CREDITS: &[&str] = &[
    "credit balance is too low",
    "insufficient_quota",
    "insufficient credits",
    "exceeded your current quota",
    "billing_error",
    "payment required",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Limit,
    NoCredits,
}

/// What this output says about the account, if anything. Credits win: an
/// account out of money often says "rate limit" too.
pub fn classify(text: &str) -> Option<Signal> {
    let lower = text.to_lowercase();
    if NO_CREDITS.iter().any(|m| lower.contains(m)) {
        return Some(Signal::NoCredits);
    }
    LIMIT.iter().any(|m| lower.contains(m)).then_some(Signal::Limit)
}

/// The reset the CLI printed with its limit — «resets 10:40pm» or «resets
/// Nov 3, 10am» — in the machine's local time, which is the zone the CLI
/// names. The next such moment after `now`.
pub fn reset_in_text(text: &str, now: DateTime<chrono::Local>) -> Option<DateTime<Utc>> {
    use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone};
    let re = regex::Regex::new(r"(?i)resets\s+(?:at\s+)?(?:(?P<mon>[a-z]{3})[a-z]*\.?\s+(?P<day>\d{1,2}),?\s+(?:at\s+)?)?(?P<h>\d{1,2})(?::(?P<m>\d{2}))?\s*(?P<ap>am|pm)").ok()?;
    let c = re.captures(text)?;
    let mut hour: u32 = c["h"].parse().ok()?;
    let minute: u32 = c.name("m").and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let pm = c["ap"].eq_ignore_ascii_case("pm");
    if hour == 12 {
        hour = 0;
    }
    if pm {
        hour += 12;
    }
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let local = |d: NaiveDate| chrono::Local.from_local_datetime(&d.and_time(time)).earliest();
    let at = match (c.name("mon"), c.name("day")) {
        (Some(mon), Some(day)) => {
            const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
            let month = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(mon.as_str()))? as u32 + 1;
            let day: u32 = day.as_str().parse().ok()?;
            let this_year = local(NaiveDate::from_ymd_opt(now.year(), month, day)?)?;
            // A date already past is next year's (a reset on Jan 2, read on Dec 30).
            if this_year < now - Duration::days(1) { local(NaiveDate::from_ymd_opt(now.year() + 1, month, day)?)? } else { this_year }
        }
        _ => {
            let today = local(now.date_naive())?;
            if today <= now { local(now.date_naive().succ_opt()?)? } else { today }
        }
    };
    Some(at.with_timezone(&Utc))
}

/// When to try again: the reset the CLI printed with the limit, else the one
/// Claude Code's status line reported for this account (plus a little
/// jitter), else a cautious half hour.
pub fn resume_at(said: &str, root: Option<&std::path::Path>, account_id: &str) -> DateTime<Utc> {
    if let Some(at) = reset_in_text(said, chrono::Local::now()) {
        // Just after the reset, not on it: the edge refuses too.
        return at + Duration::seconds(crate::auto_continue::random_jitter());
    }
    let reported = root
        .and_then(|r| crate::auto_continue::latest_rate_limits(r, account_id))
        .and_then(|limits| crate::auto_continue::resume_at(&limits, crate::auto_continue::random_jitter()))
        .filter(|at| *at > Utc::now());
    reported.unwrap_or_else(|| Utc::now() + Duration::minutes(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_cli_output_is_told_apart() {
        // Claude Code, subscription window.
        assert_eq!(classify(r#"{"type":"result","is_error":true,"result":"Claude AI usage limit reached|1761832800"}"#), Some(Signal::Limit));
        // Anthropic API, key out of money.
        assert_eq!(
            classify(r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#),
            Some(Signal::NoCredits)
        );
        // Codex, plan window and API key without quota.
        assert_eq!(classify("ERROR: You've hit your usage limit. Upgrade to Pro or try again in 2 hours."), Some(Signal::Limit));
        assert_eq!(classify(r#"{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}"#), Some(Signal::NoCredits));
        assert_eq!(classify("error: tests failed (3 failed, 9 passed)"), None);
    }

    #[test]
    fn claude_codes_current_limit_and_its_reset_are_read() {
        use chrono::{Local, TimeZone, Timelike};
        let said = "You've hit your 5-hour limit · resets 10:40pm (America/Sao_Paulo)";
        assert_eq!(classify(said), Some(Signal::Limit));
        assert_eq!(classify("You've hit your weekly limit · resets Nov 3, 10am"), Some(Signal::Limit));

        let evening = Local.with_ymd_and_hms(2026, 10, 30, 21, 45, 0).unwrap();
        let at = reset_in_text(said, evening).unwrap().with_timezone(&Local);
        assert_eq!((at.date_naive(), at.hour(), at.minute()), (evening.date_naive(), 22, 40), "later today");
        let late = Local.with_ymd_and_hms(2026, 10, 30, 23, 0, 0).unwrap();
        let at = reset_in_text(said, late).unwrap().with_timezone(&Local);
        assert_eq!((at.date_naive(), at.hour()), (late.date_naive().succ_opt().unwrap(), 22), "already past: tomorrow");
        let weekly = reset_in_text("resets Nov 3, 10am", evening).unwrap().with_timezone(&Local);
        assert_eq!((weekly.format("%m-%d %H:%M").to_string()), "11-03 10:00");
        assert_eq!(reset_in_text("resets 12am", evening).unwrap().with_timezone(&Local).hour(), 0);
        assert!(reset_in_text("no reset here", evening).is_none());
    }

    #[test]
    fn without_a_reported_reset_it_waits_half_an_hour() {
        let at = resume_at("", None, "acc");
        let mins = (at - Utc::now()).num_minutes();
        assert!((29..=30).contains(&mins));
    }
}
