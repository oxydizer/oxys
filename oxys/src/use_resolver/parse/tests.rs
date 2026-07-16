use super::parse_required_use;
use crate::use_resolver::RequiredUseExpr;

#[test]
fn parses_simple_flag_required_use() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_required_use("flag")?,
        vec![RequiredUseExpr::Flag("flag".to_owned())]
    );
    Ok(())
}

#[test]
fn parses_negated_flag_required_use() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_required_use("!flag")?,
        vec![RequiredUseExpr::Not("flag".to_owned())]
    );
    Ok(())
}

#[test]
fn parses_any_of_required_use() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_required_use("|| ( a b )")?,
        vec![RequiredUseExpr::AnyOf(vec![
            RequiredUseExpr::Flag("a".to_owned()),
            RequiredUseExpr::Flag("b".to_owned()),
        ])]
    );
    Ok(())
}

#[test]
fn parses_exactly_one_required_use() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_required_use("^^ ( a b c )")?,
        vec![RequiredUseExpr::ExactlyOne(vec![
            RequiredUseExpr::Flag("a".to_owned()),
            RequiredUseExpr::Flag("b".to_owned()),
            RequiredUseExpr::Flag("c".to_owned()),
        ])]
    );
    Ok(())
}

#[test]
fn parses_at_most_one_required_use() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_required_use("?? ( a b )")?,
        vec![RequiredUseExpr::AtMostOne(vec![
            RequiredUseExpr::Flag("a".to_owned()),
            RequiredUseExpr::Flag("b".to_owned()),
        ])]
    );
    Ok(())
}

#[test]
fn parses_conditional_required_use() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_required_use("foo? ( bar )")?,
        vec![RequiredUseExpr::IfThen(
            "foo".to_owned(),
            vec![RequiredUseExpr::Flag("bar".to_owned())],
        )]
    );
    Ok(())
}

#[test]
fn parses_nested_required_use() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_required_use("foo? ( || ( a b ) )")?,
        vec![RequiredUseExpr::IfThen(
            "foo".to_owned(),
            vec![RequiredUseExpr::AnyOf(vec![
                RequiredUseExpr::Flag("a".to_owned()),
                RequiredUseExpr::Flag("b".to_owned()),
            ])],
        )]
    );
    Ok(())
}

#[test]
fn rejects_invalid_required_use() {
    assert!(parse_required_use("|| ( a b ").is_err());
}
