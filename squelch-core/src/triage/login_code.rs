//! Structured extraction for notification copy, shared by both assessment lanes.
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub const PROMPT: &str = "Extract login_code only for a single unambiguous current one-time login or verification code in this arrival. Return null for missing or conflicting codes, quoted older messages, password-reset tokens, recovery codes, passwords, URLs, and informational security alerts. The object contains service (the short recognizable service name, at most 32 characters, copied from the sender, subject, or body) and code (copy exactly, preserving zeros, case, spaces and hyphens). Never invent either field. Keep codes out of descriptive notification text and reasons; the application formats the extracted code.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginCode {
    pub service: String,
    pub code: String,
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "anyOf": [
            {"type": "null"},
            {"type": "object", "additionalProperties": false,
             "required": ["service", "code"],
             "properties": {"service": {"type": "string"}, "code": {"type": "string"}}}
        ]
    })
}

impl LoginCode {
    /// Fail closed on malformed or hallucinated extractions. Semantic identification
    /// belongs to the assessor; source matching prevents publishing invented codes.
    pub fn notification(
        &self,
        is_auth: bool,
        sender: &str,
        subject: &str,
        body: &str,
        eligible_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Option<String> {
        static UNSAFE_SERVICE: OnceLock<regex::Regex> = OnceLock::new();
        let unsafe_service =
            UNSAFE_SERVICE.get_or_init(|| regex::Regex::new(r"[\p{Cc}\p{Cf}]").unwrap());
        let service = self.service.trim();
        let code = &self.code;
        let symbols = code.chars().filter(|c| c.is_ascii_alphanumeric()).count();
        if !is_auth
            || service.is_empty()
            || now - eligible_at >= Duration::minutes(10)
            || now < eligible_at
            || service.chars().count() > 32
            || unsafe_service.is_match(&self.service)
            || ![sender, subject, body]
                .iter()
                .any(|text| text.to_lowercase().contains(&service.to_lowercase()))
            || !(4..=12).contains(&symbols)
            || code.len() > 24
            || code.trim() != code
            || !code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-')
            || ![subject, body].iter().any(|text| {
                text.match_indices(code.as_str()).any(|(start, matched)| {
                    !text[..start]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric())
                        && !text[start + matched.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_alphanumeric())
                })
            })
        {
            return None;
        }
        Some(format!("Your {service} login code is {code}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_codes_and_requires_source_evidence() {
        for code in ["001234", "Ab9Xyz", "ABC-DEF", "123 456"] {
            let extraction = LoginCode {
                service: "Example".into(),
                code: code.into(),
            };
            assert_eq!(
                extraction.notification(
                    true,
                    "Example",
                    "",
                    &format!("Your code: {code}."),
                    Utc::now(),
                    Utc::now()
                ),
                Some(format!("Your Example login code is {code}"))
            );
            assert_eq!(
                extraction.notification(false, "Example", code, "", Utc::now(), Utc::now()),
                None
            );
            assert_eq!(
                extraction.notification(
                    true,
                    "Example",
                    "",
                    "No code here",
                    Utc::now(),
                    Utc::now()
                ),
                None
            );
            assert_eq!(
                extraction.notification(
                    true,
                    "Example",
                    "",
                    &format!("X{code}9"),
                    Utc::now(),
                    Utc::now()
                ),
                None
            );
        }
    }

    #[test]
    fn service_must_be_short_safe_and_present_in_a_source_field() {
        let now = Utc::now();
        let extraction = LoginCode {
            service: "Example".into(),
            code: "001234".into(),
        };
        for (sender, subject, body) in [
            ("noreply@EXAMPLE.com", "", "001234"),
            ("", "eXaMpLe login", "001234"),
            ("", "", "Example code: 001234"),
        ] {
            assert!(
                extraction
                    .notification(true, sender, subject, body, now, now)
                    .is_some()
            );
        }
        assert_eq!(
            extraction.notification(true, "Other", "", "001234", now, now),
            None
        );
        for service in [
            "X".repeat(33),
            "Example\u{202e}".into(),
            "Example\u{200d}".into(),
            "\u{2066}Example".into(),
            "Example\n".into(),
        ] {
            let extraction = LoginCode {
                service: service.clone(),
                code: "001234".into(),
            };
            // Even source-supported format/control characters must be rejected.
            assert_eq!(
                extraction.notification(true, &service, "", "001234", now, now),
                None
            );
        }
        let service = "X".repeat(32);
        let extraction = LoginCode {
            service: service.clone(),
            code: "001234".into(),
        };
        assert!(
            extraction
                .notification(true, &service, "", "001234", now, now)
                .is_some()
        );
    }

    #[test]
    fn codes_are_only_shown_for_arrivals_under_ten_minutes_old() {
        let now = Utc::now();
        let extraction = LoginCode {
            service: "Example".into(),
            code: "001234".into(),
        };
        for (age, allowed) in [
            (Duration::seconds(599), true),
            (Duration::minutes(10), false),
            (Duration::minutes(50), false),
            (Duration::seconds(-1), false),
        ] {
            assert_eq!(
                extraction
                    .notification(true, "Example", "", "001234", now - age, now)
                    .is_some(),
                allowed
            );
        }
    }

    #[test]
    fn rejects_malformed_extractions() {
        for (service, code) in [
            ("", "123456"),
            ("Bad\nService", "123456"),
            ("Example", "https://reset"),
            ("Example", "123"),
            ("Example", "1234567890123"),
        ] {
            let extraction = LoginCode {
                service: service.into(),
                code: code.into(),
            };
            assert_eq!(
                extraction.notification(true, "Example", code, "", Utc::now(), Utc::now()),
                None
            );
        }
    }
}
