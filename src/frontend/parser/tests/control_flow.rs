use super::*;

// Statement and control-flow tests
#[test]
fn parses_basic_statements() {
    assert_eq!(parse_statement_source(";"), Stmt::Empty);
    assert_eq!(parse_statement_source("return;"), Stmt::Return(None));
    assert_eq!(parse_statement_source("break;"), Stmt::Break);
    assert_eq!(parse_statement_source("continue;"), Stmt::Continue);
    assert_eq!(
        parse_statement_source("goto entry;"),
        Stmt::Goto("entry".to_string())
    );
}

#[test]
fn parses_compound_statement() {
    let stmt = parse_statement_source("{ int x; x = 1; }");
    let Stmt::Compound(compound) = stmt else {
        panic!("expected compound");
    };
    assert_eq!(compound.items.len(), 2);
    let BlockItem::Decl(decl) = &compound.items[0] else {
        panic!("expected declaration");
    };
    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
}

#[test]
fn parses_if_else() {
    let stmt = parse_statement_source("if (flag) x = 1; else x = 2;");
    let Stmt::If {
        cond,
        then_branch: _,
        else_branch,
    } = stmt
    else {
        panic!("expected if");
    };
    assert_eq!(cond, Expr::var("flag".to_string()));
    assert!(else_branch.is_some());
}

#[test]
fn parses_loops() {
    let while_stmt = parse_statement_source("while (x < 10) x++;");
    assert!(matches!(while_stmt, Stmt::While { .. }));

    let do_while = parse_statement_source("do x++; while (x < 10);");
    assert!(matches!(do_while, Stmt::DoWhile { .. }));

    let for_stmt = parse_statement_source("for (i = 0; i < 10; i++) i;");
    assert!(matches!(for_stmt, Stmt::For { .. }));
}

#[test]
fn parses_for_with_declaration() {
    let stmt = parse_statement_source("for (int i = 0; i < 3; i++) ;");
    let Stmt::For { init, .. } = stmt else {
        panic!("expected for");
    };
    let Some(ForInit::Decl(decl)) = init else {
        panic!("expected decl init");
    };
    assert_eq!(decl.specifiers.ty, vec![TypeSpecifier::Int]);
}

#[test]
fn parses_switch_and_case() {
    let switch_stmt = parse_statement_source("switch (x) break;");
    assert!(matches!(switch_stmt, Stmt::Switch { .. }));

    let case_stmt = parse_statement_source("case 1: break;");
    assert!(matches!(case_stmt, Stmt::Case { .. }));

    let default_stmt = parse_statement_source("default: continue;");
    assert!(matches!(default_stmt, Stmt::Default { .. }));
}

#[test]
fn rejects_invalid_case_expressions() {
    let invalid = [
        "case 1, 2: break;",
        "case y = 1: break;",
        "case foo(): break;",
    ];
    for src in invalid {
        assert!(!parse_statement_source_error(src).is_empty());
    }
}

// Postfix expression tests
#[test]
fn parses_function_calls() {
    let cases = [
        ("result = add(1, 2);", 2),
        ("result = get();", 0),
        ("result = factory()(42);", 1), // Chained call
    ];
    for (src, _) in cases {
        let stmt = parse_statement_source(src);
        assert!(matches!(stmt, Stmt::Expr(_)));
    }
}

#[test]
fn parses_array_subscript() {
    let stmt = parse_statement_source("value = arr[i + 1];");
    let Stmt::Expr(expr) = stmt else {
        panic!("expected expr stmt");
    };
    let ExprKind::Assign { right, .. } = &expr.kind else {
        panic!("expected assign");
    };
    assert!(matches!(right.kind, ExprKind::Index { .. }));
}

#[test]
fn parses_member_access() {
    let cases = [
        "value = point.x;",
        "value = node->next;",
        "value = factory().items[i].count;", // Chained
    ];
    for src in cases {
        let stmt = parse_statement_source(src);
        assert!(matches!(stmt, Stmt::Expr(_)));
    }
}

#[test]
fn parses_comma_in_call_arg() {
    let stmt = parse_statement_source("result = f((1, 2));");
    let Stmt::Expr(expr) = stmt else {
        panic!("expected expr stmt");
    };
    let ExprKind::Assign { right, .. } = &expr.kind else {
        panic!("expected assign");
    };
    let ExprKind::Call { args, .. } = &right.kind else {
        panic!("expected call");
    };
    assert_eq!(args.len(), 1);
    assert!(matches!(args[0].kind, ExprKind::Comma { .. }));
}
