use super::*;

pub(super) fn explain_required_use_violation(
    expr: &RequiredUseExpr,
    enabled_flags: &BTreeSet<String>,
) -> Option<String> {
    match expr {
        RequiredUseExpr::Flag(flag) => {
            (!enabled_flags.contains(flag)).then(|| format!("`{flag}` must be enabled"))
        }
        RequiredUseExpr::Not(flag) => enabled_flags
            .contains(flag)
            .then(|| format!("`{flag}` must be disabled")),
        RequiredUseExpr::AnyOf(items) => {
            let enabled = items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count();
            (enabled == 0).then(|| {
                format!(
                    "at least one of {} must be enabled but 0 are",
                    render_required_use_list(items)
                )
            })
        }
        RequiredUseExpr::ExactlyOne(items) => {
            let enabled = items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count();
            (enabled != 1).then(|| {
                format!(
                    "exactly one of {} must be enabled but {} are",
                    render_required_use_list(items),
                    enabled
                )
            })
        }
        RequiredUseExpr::AtMostOne(items) => {
            let enabled = items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count();
            (enabled > 1).then(|| {
                format!(
                    "at most one of {} may be enabled but {} are",
                    render_required_use_list(items),
                    enabled
                )
            })
        }
        RequiredUseExpr::IfThen(flag, items) => {
            if enabled_flags.contains(flag) {
                items
                    .iter()
                    .find_map(|item| explain_required_use_violation(item, enabled_flags))
                    .map(|reason| format!("when `{flag}` is enabled, {reason}"))
            } else {
                None
            }
        }
        RequiredUseExpr::AllOf(items) => items
            .iter()
            .find_map(|item| explain_required_use_violation(item, enabled_flags)),
    }
}

fn required_use_expr_matches(expr: &RequiredUseExpr, enabled_flags: &BTreeSet<String>) -> bool {
    match expr {
        RequiredUseExpr::Flag(flag) => enabled_flags.contains(flag),
        RequiredUseExpr::Not(flag) => !enabled_flags.contains(flag),
        RequiredUseExpr::AnyOf(items) => items
            .iter()
            .any(|item| required_use_expr_matches(item, enabled_flags)),
        RequiredUseExpr::ExactlyOne(items) => {
            items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count()
                == 1
        }
        RequiredUseExpr::AtMostOne(items) => {
            items
                .iter()
                .filter(|item| required_use_expr_matches(item, enabled_flags))
                .count()
                <= 1
        }
        RequiredUseExpr::IfThen(flag, items) => {
            !enabled_flags.contains(flag)
                || items
                    .iter()
                    .all(|item| required_use_expr_matches(item, enabled_flags))
        }
        RequiredUseExpr::AllOf(items) => items
            .iter()
            .all(|item| required_use_expr_matches(item, enabled_flags)),
    }
}

pub(super) fn render_required_use_expr(expr: &RequiredUseExpr) -> String {
    match expr {
        RequiredUseExpr::Flag(flag) => flag.clone(),
        RequiredUseExpr::Not(flag) => format!("!{flag}"),
        RequiredUseExpr::AnyOf(items) => format!("|| ( {} )", render_required_use_items(items)),
        RequiredUseExpr::ExactlyOne(items) => {
            format!("^^ ( {} )", render_required_use_items(items))
        }
        RequiredUseExpr::AtMostOne(items) => {
            format!("?? ( {} )", render_required_use_items(items))
        }
        RequiredUseExpr::IfThen(flag, items) => {
            format!("{flag}? ( {} )", render_required_use_items(items))
        }
        RequiredUseExpr::AllOf(items) => format!("( {} )", render_required_use_items(items)),
    }
}

fn render_required_use_items(items: &[RequiredUseExpr]) -> String {
    items
        .iter()
        .map(render_required_use_expr)
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_required_use_list(items: &[RequiredUseExpr]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(render_required_use_expr)
            .collect::<Vec<_>>()
            .join(", ")
    )
}
