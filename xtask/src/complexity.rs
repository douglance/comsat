use std::fs;
use std::path::Path;

use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{
    Attribute, Block, Expr, ExprIf, ExprLoop, ExprMatch, ExprWhile, ImplItem, ImplItemFn, ItemFn,
    ItemImpl, ItemMod,
};

use crate::manifest;
use crate::{Result, display_path};

const FUNCTION_BODY_LOC_LIMIT: usize = 50;
const CYCLOMATIC_LIMIT: usize = 8;
const COGNITIVE_LIMIT: usize = 10;
const NESTING_LIMIT: usize = 3;
const ARGUMENT_LIMIT: usize = 5;
const FILE_WARNING_LOC: usize = 300;
const FILE_HARD_LOC: usize = 500;

pub fn run(root: &Path) -> Result<()> {
    let files = manifest::production_rs_files(root)?;
    let mut violations = Vec::new();
    let mut warnings = Vec::new();

    for path in files {
        let source = fs::read_to_string(&path)?;
        let line_count = source.lines().count();
        if line_count > FILE_HARD_LOC {
            violations.push(format!(
                "{} has {line_count} lines, limit is {FILE_HARD_LOC}",
                display_path(root, &path).display()
            ));
        } else if line_count > FILE_WARNING_LOC {
            warnings.push(format!(
                "{} has {line_count} lines, warning threshold is {FILE_WARNING_LOC}",
                display_path(root, &path).display()
            ));
        }

        let syntax = syn::parse_file(&source)?;
        let mut visitor = FunctionVisitor::new(root, &path, &source, &mut violations);
        visitor.visit_file(&syntax);
    }

    for warning in warnings {
        eprintln!("warning: {warning}");
    }

    if violations.is_empty() {
        println!("complexity checks passed");
        Ok(())
    } else {
        Err(format!("complexity check failed:\n{}", violations.join("\n")).into())
    }
}

struct FunctionVisitor<'a> {
    root: &'a Path,
    path: &'a Path,
    source: &'a str,
    violations: &'a mut Vec<String>,
}

impl<'a> FunctionVisitor<'a> {
    const fn new(
        root: &'a Path,
        path: &'a Path,
        source: &'a str,
        violations: &'a mut Vec<String>,
    ) -> Self {
        Self {
            root,
            path,
            source,
            violations,
        }
    }

    fn check_function(&mut self, name: &str, args: usize, block: &Block, span: Span) {
        let metrics = FunctionMetrics::measure(block);
        let checks = [
            MetricCheck::new("body LOC", metrics.body_loc, FUNCTION_BODY_LOC_LIMIT),
            MetricCheck::new(
                "cyclomatic complexity",
                metrics.cyclomatic,
                CYCLOMATIC_LIMIT,
            ),
            MetricCheck::new("cognitive complexity", metrics.cognitive, COGNITIVE_LIMIT),
            MetricCheck::new("nesting depth", metrics.max_nesting, NESTING_LIMIT),
            MetricCheck::new("arguments", args, ARGUMENT_LIMIT),
        ];
        for check in checks {
            self.check_metric(name, check, span);
        }
    }

    fn check_metric(&mut self, name: &str, check: MetricCheck, span: Span) {
        if check.value <= check.limit || self.has_exception(check.metric, span) {
            return;
        }
        let line = span.start().line;
        let metric = check.metric;
        let value = check.value;
        let limit = check.limit;
        self.violations.push(format!(
            "{}:{line} `{name}` {metric} is {value}, limit is {limit}",
            display_path(self.root, self.path).display()
        ));
    }

    fn has_exception(&self, metric: &str, span: Span) -> bool {
        let line = span.start().line;
        let start = line.saturating_sub(4);
        self.source
            .lines()
            .enumerate()
            .skip(start)
            .take(line.saturating_sub(start))
            .any(|(_, source_line)| exception_covers(source_line, metric))
    }
}

impl<'ast> Visit<'ast> for FunctionVisitor<'_> {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        for item in &node.items {
            if let ImplItem::Fn(function) = item {
                self.visit_impl_item_fn(function);
            }
        }
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        let name = node.sig.ident.to_string();
        self.check_function(&name, node.sig.inputs.len(), &node.block, node.span());
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        let name = node.sig.ident.to_string();
        self.check_function(&name, node.sig.inputs.len(), &node.block, node.span());
        syn::visit::visit_impl_item_fn(self, node);
    }
}

#[derive(Clone, Copy)]
struct MetricCheck {
    metric: &'static str,
    value: usize,
    limit: usize,
}

impl MetricCheck {
    const fn new(metric: &'static str, value: usize, limit: usize) -> Self {
        Self {
            metric,
            value,
            limit,
        }
    }
}

#[derive(Default)]
struct FunctionMetrics {
    body_loc: usize,
    cyclomatic: usize,
    cognitive: usize,
    max_nesting: usize,
}

impl FunctionMetrics {
    fn measure(block: &Block) -> Self {
        let mut counter = MetricCounter {
            metrics: Self {
                body_loc: span_lines(block.span()),
                cyclomatic: 1,
                cognitive: 0,
                max_nesting: 0,
            },
            nesting: 0,
        };
        counter.visit_block(block);
        counter.metrics
    }
}

struct MetricCounter {
    metrics: FunctionMetrics,
    nesting: usize,
}

impl MetricCounter {
    fn enter_control<F>(&mut self, visit: F)
    where
        F: FnOnce(&mut Self),
    {
        self.metrics.cyclomatic += 1;
        self.metrics.cognitive += 1 + self.nesting;
        self.nesting += 1;
        self.metrics.max_nesting = self.metrics.max_nesting.max(self.nesting);
        visit(self);
        self.nesting -= 1;
    }
}

impl<'ast> Visit<'ast> for MetricCounter {
    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        self.enter_control(|visitor| syn::visit::visit_expr_if(visitor, node));
    }

    fn visit_expr_while(&mut self, node: &'ast ExprWhile) {
        self.enter_control(|visitor| syn::visit::visit_expr_while(visitor, node));
    }

    fn visit_expr_loop(&mut self, node: &'ast ExprLoop) {
        self.enter_control(|visitor| syn::visit::visit_expr_loop(visitor, node));
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        let arm_count = node.arms.len();
        if arm_count > 0 {
            self.metrics.cyclomatic += arm_count;
            self.metrics.cognitive += arm_count + self.nesting;
        }
        self.nesting += 1;
        self.metrics.max_nesting = self.metrics.max_nesting.max(self.nesting);
        syn::visit::visit_expr_match(self, node);
        self.nesting -= 1;
    }

    fn visit_expr(&mut self, node: &'ast Expr) {
        match node {
            Expr::Binary(binary) if matches!(binary.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) => {
                self.metrics.cyclomatic += 1;
            }
            _ => {}
        }
        syn::visit::visit_expr(self, node);
    }
}

fn span_lines(span: Span) -> usize {
    let start = span.start().line;
    let end = span.end().line;
    end.saturating_sub(start).saturating_add(1)
}

fn exception_covers(source_line: &str, metric: &str) -> bool {
    source_line.contains("comsat-allow-complexity")
        && source_line.contains(metric)
        && source_line.contains("reason:")
}

fn has_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg") && attr.meta.to_token_stream().to_string().contains("test")
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use syn::visit::Visit;

    use crate::complexity::{FunctionVisitor, exception_covers};

    #[test]
    fn exception_without_reason_is_rejected() {
        let line = "// comsat-allow-complexity body LOC";

        assert!(!exception_covers(line, "body LOC"));
    }

    #[test]
    fn excessive_arguments_are_reported() {
        let source = "fn too_many(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) {}";
        let syntax = syn::parse_file(source).expect("test source parses");
        let mut violations = Vec::new();
        let mut visitor = FunctionVisitor::new(
            Path::new("."),
            Path::new("fixture.rs"),
            source,
            &mut violations,
        );

        visitor.visit_file(&syntax);

        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("arguments"))
        );
    }
}
