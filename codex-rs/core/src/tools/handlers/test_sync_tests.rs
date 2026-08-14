use super::*;

#[test]
fn timing_label_is_bounded_before_it_is_echoed() {
    let expected_label = "x".repeat(MAX_TIMING_LABEL_BYTES);
    let accepted = serde_json::from_value::<TestSyncArgs>(serde_json::json!({
        "timing_label": expected_label,
    }))
    .expect("the maximum timing label should be accepted");
    assert_eq!(
        accepted.timing_label.as_deref(),
        Some(expected_label.as_str())
    );

    let rejected = serde_json::from_value::<TestSyncArgs>(serde_json::json!({
        "timing_label": "x".repeat(MAX_TIMING_LABEL_BYTES + 1),
    }));
    assert!(
        rejected.is_err(),
        "oversized timing labels must be rejected"
    );
}
