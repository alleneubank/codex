use super::*;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;

#[test]
fn enter_worktree_schema_requires_exactly_one_selector() {
    let ToolSpec::Function(tool) = create_enter_worktree_tool() else {
        panic!("enter_worktree should be a function tool");
    };

    let properties = tool
        .parameters
        .properties
        .as_ref()
        .expect("enter_worktree parameters should define properties");
    assert!(properties.contains_key("name"));
    assert!(properties.contains_key("path"));

    let required_by_variant = tool
        .parameters
        .one_of
        .as_ref()
        .expect("enter_worktree should require one selector")
        .iter()
        .map(|schema| {
            schema
                .required
                .as_ref()
                .expect("selector variant should have required fields")
                .clone()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        required_by_variant,
        vec![vec!["name".to_string()], vec!["path".to_string()]]
    );

    let variant_properties = tool
        .parameters
        .one_of
        .as_ref()
        .expect("enter_worktree should require one selector")
        .iter()
        .map(|schema| {
            schema
                .properties
                .as_ref()
                .expect("selector variant should define properties")
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        variant_properties,
        vec![vec!["name".to_string()], vec!["path".to_string()]]
    );
}
