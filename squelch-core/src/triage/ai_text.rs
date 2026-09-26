//! Cheap stylometric hints that a message body was machine-written.
//!
//! These are evidence for the triage model, never a verdict. People send
//! AI-drafted mail too, so nothing here decides placement: the model weighs
//! the hints against who sent the message and why, and the ranking penalty it
//! feeds fades out for mail that is personally relevant or needs action.
use serde::Serialize;

/// Stock phrasing that assistant-drafted outreach leans on. Lowercase; matched
/// against the lowercased, quote-stripped body.
const STOCK_PHRASES: &[(&str, &str)] = &[
    ("i hope this email finds you well", "stock_greeting"),
    ("i hope this message finds you well", "stock_greeting"),
    ("i hope you're doing well", "stock_greeting"),
    ("i hope you are doing well", "stock_greeting"),
    ("i wanted to reach out", "stock_opener"),
    ("i'm reaching out", "stock_opener"),
    ("i am reaching out", "stock_opener"),
    ("i came across your", "stock_opener"),
    ("delve", "assistant_vocabulary"),
    ("tapestry", "assistant_vocabulary"),
    ("testament to", "assistant_vocabulary"),
    ("in today's fast-paced", "assistant_vocabulary"),
    ("ever-evolving", "assistant_vocabulary"),
    ("navigate the complexities", "assistant_vocabulary"),
    ("seamlessly integrate", "assistant_vocabulary"),
    ("unlock the full potential", "assistant_vocabulary"),
    ("game-changer", "assistant_vocabulary"),
    ("elevate your", "assistant_vocabulary"),
    ("it's worth noting", "assistant_vocabulary"),
    ("it is worth noting", "assistant_vocabulary"),
    ("don't hesitate to reach out", "stock_closer"),
    ("do not hesitate to reach out", "stock_closer"),
    ("feel free to reach out", "stock_closer"),
    ("looking forward to hearing your thoughts", "stock_closer"),
    ("let me know if you have any questions", "stock_closer"),
];

/// Unfilled template slots and leaked assistant chatter: near-certain tells.
const LEAKS: &[&str] = &[
    "[your name]",
    "[recipient",
    "[company name]",
    "[first name]",
    "{first_name}",
    "{{first_name}}",
    "as an ai language model",
    "as an ai assistant",
    "here's a draft",
    "here is a draft",
    "certainly! here",
    "sure! here",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AiTextSignals {
    /// Heuristic likelihood on [0, 1]. Weak by design: the model decides.
    pub score: f64,
    /// Stable marker names explaining the score. Never body quotations.
    pub markers: Vec<&'static str>,
    /// Words examined after quoted history was removed.
    pub words: usize,
}

/// Only the author's own text counts: quoted replies and forwarded history
/// belong to someone else.
fn own_text(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if (lower.starts_with("on ") && lower.ends_with("wrote:"))
            || lower.starts_with("-----original message")
            || lower.starts_with("---------- forwarded message")
        {
            break;
        }
        if trimmed.starts_with('>') {
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    out
}

pub fn assess(body: &str) -> AiTextSignals {
    let text = own_text(body);
    let lower = text.to_lowercase();
    let words = lower.split_whitespace().count();
    let mut markers: Vec<&'static str> = Vec::new();
    let mut score: f64 = 0.0;
    // Very short text carries too little style to judge.
    if words < 25 {
        return AiTextSignals {
            score: 0.0,
            markers,
            words,
        };
    }
    if LEAKS.iter().any(|leak| lower.contains(leak)) {
        markers.push("template_or_assistant_leak");
        score += 0.6;
    }
    let mut phrase_hits = 0;
    for (phrase, marker) in STOCK_PHRASES {
        if lower.contains(phrase) {
            phrase_hits += 1;
            if !markers.contains(marker) {
                markers.push(marker);
            }
        }
    }
    score += (phrase_hits as f64 * 0.12).min(0.48);
    // Em dashes are common in assistant prose and rare in typed email, where
    // people reach for a hyphen. Normalize per 100 words.
    let dashes = text.matches('\u{2014}').count();
    if dashes as f64 / words as f64 * 100.0 >= 1.0 && dashes >= 2 {
        markers.push("em_dash_density");
        score += 0.15;
    }
    // Bolded pseudo-headings and bullet lists in a personal note are a
    // formatting habit of chat output pasted into mail.
    let bold_heads = text
        .lines()
        .filter(|l| l.starts_with("**") && l.contains(":**"))
        .count();
    if bold_heads >= 2 {
        markers.push("markdown_headings");
        score += 0.15;
    }
    // Uniform sentence length ("burstiness" is low in generated text). Needs
    // enough sentences to mean anything.
    let lengths: Vec<f64> = lower
        .split(['.', '!', '?'])
        .map(|s| s.split_whitespace().count() as f64)
        .filter(|&n| n >= 3.0)
        .collect();
    if lengths.len() >= 6 {
        let mean = lengths.iter().sum::<f64>() / lengths.len() as f64;
        let var = lengths.iter().map(|n| (n - mean).powi(2)).sum::<f64>() / lengths.len() as f64;
        if mean > 0.0 && var.sqrt() / mean < 0.3 {
            markers.push("uniform_sentence_length");
            score += 0.1;
        }
    }
    AiTextSignals {
        score: score.clamp(0.0, 1.0),
        markers,
        words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terse_human_mail_scores_nothing() {
        let s = assess("running 10 late, grab a table?\n\n- j");
        assert_eq!(s.score, 0.0);
        assert!(s.markers.is_empty());
    }

    #[test]
    fn assistant_outreach_scores_high_with_named_markers() {
        let body = "Hi Sam,\n\nI hope this email finds you well. I wanted to reach out \
            because I came across your work and it's truly a testament to what a small team \
            can do \u{2014} and in today's fast-paced landscape that matters. Our platform can \
            seamlessly integrate with your stack \u{2014} and unlock the full potential of your \
            data.\n\n**Why it matters:**\nSpeed.\n**What's next:**\nA call.\n\nDon't hesitate \
            to reach out if you have questions.\n\nBest,\n[Your Name]";
        let s = assess(body);
        assert!(s.score >= 0.9, "{s:?}");
        for marker in [
            "template_or_assistant_leak",
            "stock_greeting",
            "stock_opener",
            "assistant_vocabulary",
            "stock_closer",
            "em_dash_density",
            "markdown_headings",
        ] {
            assert!(s.markers.contains(&marker), "{marker}: {s:?}");
        }
    }

    #[test]
    fn quoted_history_is_not_the_authors_text() {
        let body = "Sounds good, see you thursday. I'll bring the contract printouts and \
            we can go over the numbers together before the meeting starts with everyone.\n\n\
            On Mon, Sep 21, 2026 at 9:00 AM Vendor <v@example.com> wrote:\n\
            > I hope this email finds you well. I wanted to reach out to delve into a \
            game-changer. [Your Name]";
        let s = assess(body);
        assert_eq!(s.score, 0.0, "{s:?}");
    }

    #[test]
    fn score_is_bounded() {
        let body = LEAKS.join(" ")
            + " "
            + &STOCK_PHRASES
                .iter()
                .map(|(p, _)| *p)
                .collect::<Vec<_>>()
                .join(". ");
        let s = assess(&body);
        assert!((0.0..=1.0).contains(&s.score));
    }
}
