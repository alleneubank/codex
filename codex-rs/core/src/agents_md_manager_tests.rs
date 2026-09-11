use super::*;

#[test]
fn combined_instruction_limit_includes_replacement_notice_and_markers() {
    let loaded = LoadedAgentsMd::from_text_for_testing(
        "x".repeat(approx_bytes_for_tokens(MAX_COMBINED_INSTRUCTIONS_TOKENS)),
    );

    let err = validate_combined_instruction_size(Some(&loaded))
        .expect_err("rendered replacement should exceed the combined instruction limit");

    assert!(
        err.to_string()
            .contains("combined AGENTS.md instructions exceed")
    );
}
