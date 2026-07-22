use super::parse_required_use;
use crate::use_resolver::RequiredUseExpr;

#[test]
fn parses_required_use_expressions() -> Result<(), Box<dyn std::error::Error>> {
    let cases: &[(&str, Vec<RequiredUseExpr>)] = &[
        ("flag", vec![RequiredUseExpr::Flag("flag".to_owned())]),
        ("!flag", vec![RequiredUseExpr::Not("flag".to_owned())]),
        (
            "|| ( a b )",
            vec![RequiredUseExpr::AnyOf(vec![
                RequiredUseExpr::Flag("a".to_owned()),
                RequiredUseExpr::Flag("b".to_owned()),
            ])],
        ),
        (
            "^^ ( a b c )",
            vec![RequiredUseExpr::ExactlyOne(vec![
                RequiredUseExpr::Flag("a".to_owned()),
                RequiredUseExpr::Flag("b".to_owned()),
                RequiredUseExpr::Flag("c".to_owned()),
            ])],
        ),
        (
            "?? ( a b )",
            vec![RequiredUseExpr::AtMostOne(vec![
                RequiredUseExpr::Flag("a".to_owned()),
                RequiredUseExpr::Flag("b".to_owned()),
            ])],
        ),
        (
            "foo? ( bar )",
            vec![RequiredUseExpr::IfThen(
                "foo".to_owned(),
                vec![RequiredUseExpr::Flag("bar".to_owned())],
            )],
        ),
        (
            "foo? ( || ( a b ) )",
            vec![RequiredUseExpr::IfThen(
                "foo".to_owned(),
                vec![RequiredUseExpr::AnyOf(vec![
                    RequiredUseExpr::Flag("a".to_owned()),
                    RequiredUseExpr::Flag("b".to_owned()),
                ])],
            )],
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(
            parse_required_use(input)?,
            *expected,
            "parse_required_use({input:?})"
        );
    }
    Ok(())
}

#[test]
fn rejects_invalid_required_use() {
    assert!(parse_required_use("|| ( a b ").is_err());
}
