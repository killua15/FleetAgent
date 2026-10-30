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

/// When to try again: the reset Claude Code's status line reported for this
/// account (plus a little jitter), or a cautious half hour when no CLI of the
/// account has reported one.
pub fn resume_at(root: Option<&std::path::Path>, account_id: &str) -> DateTime<Utc> {
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
    fn without_a_reported_reset_it_waits_half_an_hour() {
        let at = resume_at(None, "acc");
        let mins = (at - Utc::now()).num_minutes();
        assert!((29..=30).contains(&mins));
    }
}
