//! Structured extraction for notification copy, shared by both assessment lanes.
use serde::{Deserialize, Serialize};

pub const PROMPT: &str = "Extract login_code only for a single unambiguous current one-time login or verification code in this arrival. Return null for missing or conflicting codes, quoted older messages, password-reset tokens, recovery codes, passwords, URLs, and informational security alerts. The object contains service (the short recognizable service name supported by this email) and code (copy exactly, preserving zeros, case, spaces and hyphens). Never invent either field. Keep codes out of descriptive notification text and reasons; the application formats the extracted code.";

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
    pub fn notification(&self, is_auth: bool, subject: &str, body: &str) -> Option<String> {
        let service = self.service.trim();
        let code = &self.code;
        let symbols = code.chars().filter(|c| c.is_ascii_alphanumeric()).count();
        if !is_auth
            || service.is_empty()
            || service.chars().count() > 60
            || service.chars().any(|c| c.is_control())
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
                extraction.notification(true, "", &format!("Your code: {code}.")),
                Some(format!("Your Example login code is {code}"))
            );
            assert_eq!(extraction.notification(false, code, ""), None);
            assert_eq!(extraction.notification(true, "", "No code here"), None);
            assert_eq!(
                extraction.notification(true, "", &format!("X{code}9")),
                None
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
            assert_eq!(extraction.notification(true, code, ""), None);
        }
    }
}
