#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PasteDecision {
    Pasted,
    CopiedOnly,
    Failed,
}

pub(crate) fn with_registered_suppression<R, E>(
    register: impl FnOnce() -> Result<(), E>,
    write: impl FnOnce() -> Result<R, E>,
) -> Result<R, E> {
    register()?;
    write()
}

pub(crate) fn should_prompt_accessibility(trusted: bool, prompted: bool) -> bool {
    !trusted && !prompted
}

pub(crate) fn target_is_valid(
    pid: Option<i32>,
    current_pid: i32,
    target_bundle: Option<&str>,
    current_bundle: Option<&str>,
) -> bool {
    let Some(pid) = pid else { return false };
    if pid <= 0 || pid == current_pid {
        return false;
    }
    match (target_bundle, current_bundle) {
        (Some(target), Some(current)) => target != current,
        _ => true,
    }
}

pub(crate) fn bundle_matches(expected: Option<&str>, actual: Option<&str>) -> bool {
    match (expected, actual) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => true,
    }
}

pub(crate) fn decide_outcome(
    write_ok: bool,
    trusted: bool,
    target_available: bool,
    activation_ok: bool,
    injection_ok: bool,
) -> PasteDecision {
    if !write_ok {
        return PasteDecision::Failed;
    }
    if !trusted || !target_available || !activation_ok || !injection_ok {
        return PasteDecision::CopiedOnly;
    }
    PasteDecision::Pasted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppression_is_registered_before_writer() {
        let events = std::cell::RefCell::new(Vec::new());
        let result = with_registered_suppression(
            || {
                events.borrow_mut().push("register");
                Ok::<(), ()>(())
            },
            || {
                events.borrow_mut().push("writer");
                Ok::<_, ()>(1)
            },
        );
        assert_eq!(result, Ok(1));
        assert_eq!(*events.borrow(), ["register", "writer"]);
    }

    #[test]
    fn accessibility_decisions_and_prompt_once() {
        assert!(should_prompt_accessibility(false, false));
        assert!(!should_prompt_accessibility(false, true));
        assert!(!should_prompt_accessibility(true, false));
    }

    #[test]
    fn target_decisions_reject_missing_self_and_bundle_mismatch() {
        assert!(target_is_valid(
            Some(42),
            7,
            Some("com.apple.Finder"),
            Some("com.suisui.jieone")
        ));
        assert!(!target_is_valid(None, 7, None, None));
        assert!(!target_is_valid(Some(7), 7, None, None));
        assert!(!target_is_valid(
            Some(42),
            7,
            Some("com.suisui.jieone"),
            Some("com.suisui.jieone")
        ));
        assert!(bundle_matches(
            Some("com.apple.Finder"),
            Some("com.apple.Finder")
        ));
        assert!(!bundle_matches(Some("com.apple.Finder"), Some("com.other")));
    }

    #[test]
    fn activation_and_restore_outcomes_fall_back_safely() {
        assert_eq!(
            decide_outcome(false, true, true, true, true),
            PasteDecision::Failed
        );
        assert_eq!(
            decide_outcome(true, false, true, true, true),
            PasteDecision::CopiedOnly
        );
        assert_eq!(
            decide_outcome(true, true, false, true, true),
            PasteDecision::CopiedOnly
        );
        assert_eq!(
            decide_outcome(true, true, true, false, true),
            PasteDecision::CopiedOnly
        );
        assert_eq!(
            decide_outcome(true, true, true, true, true),
            PasteDecision::Pasted
        );
    }
}
