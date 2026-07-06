use super::*;
use codex_protocol::protocol::GuardianAssessmentOutcome;
use pretty_assertions::assert_eq;

#[test]
fn low_risk_approval_omits_unknown_authorization_from_warning() {
    let assessment = GuardianAssessment {
        risk_level: GuardianRiskLevel::Low,
        user_authorization: GuardianUserAuthorization::Unknown,
        outcome: GuardianAssessmentOutcome::Allow,
        rationale: "Auto-review returned a low-risk allow decision.".to_string(),
    };

    assert_eq!(
        guardian_review_warning_message(&assessment, /*approved*/ true),
        "Automatic approval review approved (risk: low): Auto-review returned a low-risk allow decision."
    );
}

#[test]
fn denial_keeps_unknown_authorization_in_warning() {
    let assessment = GuardianAssessment {
        risk_level: GuardianRiskLevel::High,
        user_authorization: GuardianUserAuthorization::Unknown,
        outcome: GuardianAssessmentOutcome::Deny,
        rationale: "Auto-review returned a deny decision without a rationale.".to_string(),
    };

    assert_eq!(
        guardian_review_warning_message(&assessment, /*approved*/ false),
        "Automatic approval review denied (risk: high, authorization: unknown): Auto-review returned a deny decision without a rationale."
    );
}
