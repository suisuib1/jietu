use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum QuickPasteReason {
    ClipboardRestoreFailed,
    TargetUnavailable,
    TargetActivationFailed,
    AccessibilityRequired,
    InputInjectionFailed,
    EventCreationFailed,
    HistoryUpdateFailed,
    HistoryWindowFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum QuickPasteOutcome {
    Pasted,
    CopiedOnly { reason: QuickPasteReason },
    Failed { reason: QuickPasteReason },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_serialization_is_structured_and_stable() {
        assert_eq!(
            serde_json::to_value(QuickPasteOutcome::Pasted).unwrap(),
            serde_json::json!({ "kind": "pasted" })
        );
        assert_eq!(
            serde_json::to_value(QuickPasteOutcome::CopiedOnly {
                reason: QuickPasteReason::AccessibilityRequired,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "copiedOnly",
                "reason": "accessibilityRequired"
            })
        );
        assert_eq!(
            serde_json::to_value(QuickPasteOutcome::Failed {
                reason: QuickPasteReason::ClipboardRestoreFailed,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "failed",
                "reason": "clipboardRestoreFailed"
            })
        );
    }
}
