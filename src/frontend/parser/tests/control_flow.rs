use super::*;

// Statement and control-flow tests
#[test]
fn parses_basic_statements() {
    assert!(matches!(parse_statement_source(";").kind, StmtKind::Empty));
    assert!(matches!(
        parse_statement_source("return;").kind,
        StmtKind::Return(None)
    ));
    assert!(matches!(
        parse_statement_source("break;").kind,
        StmtKind::Break
    ));
    assert!(matches!(
        parse_statement_source("continue;").kind,
        StmtKind::Continue
    ));

    let goto_stmt = parse_statement_source("goto entry;");
    let StmtKind::Goto(label) = goto_stmt.kind else {
        panic!("expected goto");
    };
    assert_eq!(label, "entry");
}

#[test]
fn parses_compound_statement() {
    let stmt = parse_statement_source("{ int x; x = 1; }");
    let StmtKind::Compound(compound) = stmt.kind else {
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
    let StmtKind::If {
        cond,
        then_branch: _,
        else_branch,
    } = stmt.kind
    else {
        panic!("expected if");
    };
    assert_eq!(cond, Expr::var("flag".to_string()));
    assert!(else_branch.is_some());
}

#[test]
fn parses_loops() {
    let while_stmt = parse_statement_source("while (x < 10) x++;");
    assert!(matches!(while_stmt.kind, StmtKind::While { .. }));

    let do_while = parse_statement_source("do x++; while (x < 10);");
    assert!(matches!(do_while.kind, StmtKind::DoWhile { .. }));

    let for_stmt = parse_statement_source("for (i = 0; i < 10; i++) i;");
    assert!(matches!(for_stmt.kind, StmtKind::For { .. }));
}

#[test]
fn parses_for_with_declaration() {
    let stmt = parse_statement_source("for (int i = 0; i < 3; i++) ;");
    let StmtKind::For { init, .. } = stmt.kind else {
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
    assert!(matches!(switch_stmt.kind, StmtKind::Switch { .. }));

    let case_stmt = parse_statement_source("case 1: break;");
    assert!(matches!(case_stmt.kind, StmtKind::Case { .. }));

    let default_stmt = parse_statement_source("default: continue;");
    assert!(matches!(default_stmt.kind, StmtKind::Default { .. }));
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

#[test]
fn parses_case_sizeof_with_abstract_array_type() {
    let case_stmt = parse_statement_source("case sizeof(int[3]): break;");
    let StmtKind::Case { expr, .. } = case_stmt.kind else {
        panic!("expected case");
    };
    let ExprKind::SizeofType(ty) = expr.kind else {
        panic!("expected sizeof(type)");
    };
    let declarator = ty.declarator.as_ref().expect("array declarator expected");
    let DirectDeclaratorKind::Array { size, .. } = &declarator.direct.kind else {
        panic!("expected array declarator");
    };
    assert_eq!(size.as_ref(), &ArraySize::Expr(Expr::int(3)));
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
        assert!(matches!(stmt.kind, StmtKind::Expr(_)));
    }
}

#[test]
fn parses_array_subscript() {
    let stmt = parse_statement_source("value = arr[i + 1];");
    let StmtKind::Expr(expr) = stmt.kind else {
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
        assert!(matches!(stmt.kind, StmtKind::Expr(_)));
    }
}

#[test]
fn parses_comma_in_call_arg() {
    let stmt = parse_statement_source("result = f((1, 2));");
    let StmtKind::Expr(expr) = stmt.kind else {
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

#[test]
fn statement_span_includes_semicolon() {
    let src = "return 1;";
    let stmt = parse_statement_source(src);
    assert_eq!(stmt.span.start, 0);
    assert_eq!(stmt.span.end, src.len());
}

#[test]
fn compound_statement_span_includes_braces() {
    let src = "{ ; }";
    let stmt = parse_statement_source(src);
    let StmtKind::Compound(compound) = &stmt.kind else {
        panic!("expected compound");
    };
    assert_eq!(stmt.span.start, 0);
    assert_eq!(stmt.span.end, src.len());
    assert_eq!(compound.span.start, 0);
    assert_eq!(compound.span.end, src.len());
}

#[test]
fn prefix_and_postfix_expression_spans_cover_operators() {
    let pre = parse_statement_source("++x;");
    let StmtKind::Expr(pre_expr) = pre.kind else {
        panic!("expected expression statement");
    };
    assert_eq!(pre_expr.span.start, 0);
    assert_eq!(pre_expr.span.end, 3);

    let post = parse_statement_source("x++;");
    let StmtKind::Expr(post_expr) = post.kind else {
        panic!("expected expression statement");
    };
    assert_eq!(post_expr.span.start, 0);
    assert_eq!(post_expr.span.end, 3);
}
