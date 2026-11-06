use super::{PoolError, resume_failure};

#[test]
fn incompatible_configuration_is_not_a_resume_state_mismatch() {
    assert_eq!(
        resume_failure(&PoolError::Configuration("finite request_timeout required".to_owned())),
        "configuration"
    );
    assert_eq!(
        resume_failure(&PoolError::Protocol("restored state differs".into())),
        "mismatch"
    );
}
