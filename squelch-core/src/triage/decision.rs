//! Model-owned semantic decisions. Defaults support construction; validation is
//! mandatory before a decision becomes authoritative.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

macro_rules! decision_enum {
    ($name:ident { $first:ident $(, $variant:ident)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { #[default] $first, $($variant),* }
    };
}
decision_enum!(EmailKind {
    General,
    Correspondence,
    Editorial,
    Promotional,
    Bill,
    Receipt,
    FinancialUpdate,
    Delivery,
    EventReservation,
    AccountService,
    AuthenticationSecurity
});
decision_enum!(MessageDestination { Reading });
decision_enum!(AttentionState {
    Informational,
    NeedsUser,
    WaitingOnOthers,
    Resolved
});
decision_enum!(ActionKind {
    Review,
    Reply,
    Pay,
    Decide,
    Attend,
    Other
});
decision_enum!(AuthKind {
    Otp,
    PasswordReset,
    SignInLink,
    Verification,
    LoginAlert,
    SecurityAlert
});

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub message_id: i64,
    /// A field or attachment location, never a quoted credential.
    pub location: String,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuthAssessment {
    pub kinds: Vec<AuthKind>,
    pub evidence: Vec<EvidenceRef>,
}
impl AuthAssessment {
    pub fn is_auth(&self) -> bool {
        !self.kinds.is_empty()
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccessAssessment {
    pub restricted: bool,
    pub reason: String,
    pub evidence: Vec<EvidenceRef>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SupportedTime {
    /// ISO date or RFC3339 timestamp. Do not invent a time for a date-only fact.
    pub value: String,
    pub timezone: Option<String>,
    pub source_message_id: i64,
    pub interpreted_relative: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttentionFactors {
    pub urgency: f64,
    pub action_need: f64,
    pub personal_relevance: f64,
    pub importance: f64,
    pub attention_at: Option<SupportedTime>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttentionAction {
    /// Existing action ID when updating; the executor assigns new IDs.
    pub id: Option<String>,
    pub kind: ActionKind,
    pub description: String,
    pub resolved: bool,
    pub due: Option<SupportedTime>,
    pub evidence: Vec<EvidenceRef>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThreadAttentionDecision {
    pub show_in_fye: bool,
    pub state: AttentionState,
    pub actions: Vec<AttentionAction>,
    pub summary: String,
    pub factors: AttentionFactors,
    pub relevant_message_ids: Vec<i64>,
    pub evidence: Vec<EvidenceRef>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotificationAdvice {
    pub importance: u8,
    pub title: String,
    pub body: String,
    pub reason: String,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleException {
    pub rule_id: i64,
    pub reason: String,
    pub evidence: Vec<EvidenceRef>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RelatedAttentionUpdate {
    pub thread_id: String,
    pub expected_revision: i64,
    pub attention: ThreadAttentionDecision,
    pub evidence: Vec<EvidenceRef>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RevisitRequest {
    pub at: DateTime<Utc>,
    pub reason: String,
}

/// Explicit facts only. No record proposal implicitly closes an obligation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordProposal {
    Bill {
        merchant: String,
        amount: Option<f64>,
        currency: Option<String>,
        due: Option<SupportedTime>,
        autopay: Option<bool>,
        evidence: Vec<EvidenceRef>,
    },
    Receipt {
        merchant: String,
        amount: Option<f64>,
        currency: Option<String>,
        evidence: Vec<EvidenceRef>,
    },
    Delivery {
        carrier: Option<String>,
        tracking_number: Option<String>,
        status: String,
        evidence: Vec<EvidenceRef>,
    },
    Event {
        title: String,
        start: Option<SupportedTime>,
        location: Option<String>,
        evidence: Vec<EvidenceRef>,
    },
    FinancialUpdate {
        institution: String,
        description: String,
        evidence: Vec<EvidenceRef>,
    },
}
impl RecordProposal {
    pub fn evidence(&self) -> &[EvidenceRef] {
        match self {
            Self::Bill { evidence, .. }
            | Self::Receipt { evidence, .. }
            | Self::Delivery { evidence, .. }
            | Self::Event { evidence, .. }
            | Self::FinancialUpdate { evidence, .. } => evidence,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageDecision {
    pub kinds: Vec<EmailKind>,
    pub destinations: Vec<MessageDestination>,
    pub summary: String,
    pub reason: String,
    pub auth: AuthAssessment,
    pub external_access: AccessAssessment,
    pub attention: ThreadAttentionDecision,
    pub records: Vec<RecordProposal>,
    pub related_updates: Vec<RelatedAttentionUpdate>,
    pub rule_exceptions: Vec<RuleException>,
    pub notification: NotificationAdvice,
    pub revisit: Option<RevisitRequest>,
}
