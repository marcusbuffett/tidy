//! Static evaluation of literal JS/TS values (for `knip.ts`-style config files).

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_parser::Parser;
use oxc_span::SourceType;
use rustc_hash::FxHashMap;
use serde_json::{Map, Value};
use std::path::Path;

/// Find the default export of a module and evaluate it as a JSON literal.
pub fn default_export_literal(path: &Path, source: &str) -> Option<Value> {
    let st = SourceType::from_path(path).unwrap_or_default();
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, st).parse();
    let program = &ret.program;
    // top-level const/let bindings → literal values (for `const config = {...}; export default config;`)
    let mut bindings: FxHashMap<String, &Expression> = FxHashMap::default();
    let collect = |decl: &'_ VariableDeclaration<'_>, bindings: &mut FxHashMap<String, &Expression>| {
        for d in &decl.declarations {
            if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init) {
                // SAFETY-free: references live as long as `program`
                let init: &Expression = unsafe { std::mem::transmute(init) };
                bindings.insert(id.name.to_string(), init);
            }
        }
    };
    for stmt in &program.body {
        match stmt {
            Statement::VariableDeclaration(v) => collect(v, &mut bindings),
            Statement::ExportDeclaration(e) => {
                if let Declaration::VariableDeclaration(v) = &e.declaration {
                    collect(v, &mut bindings);
                }
            }
            _ => {}
        }
    }
    let mut result = None;
    for stmt in &program.body {
        match stmt {
            Statement::ExportDefaultDeclaration(d) => {
                if let Some(e) = d.declaration.as_expression() {
                    result = eval(e, &bindings, 0);
                }
            }
            Statement::ExpressionStatement(es) => {
                // module.exports = {...}
                if let Expression::AssignmentExpression(a) = &es.expression {
                    if let AssignmentTarget::StaticMemberExpression(m) = &a.left {
                        if m.property.name == "exports" {
                            result = eval(&a.right, &bindings, 0);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if result.is_none() {
        // `export const config = {...}` without default export
        if let Some(e) = bindings.get("config") {
            result = eval(e, &bindings, 0);
        }
    }
    result
}

fn eval(e: &Expression, bindings: &FxHashMap<String, &Expression>, depth: usize) -> Option<Value> {
    if depth > 32 {
        return None;
    }
    match e {
        Expression::StringLiteral(s) => Some(Value::String(s.value.to_string())),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => {
            Some(Value::String(t.quasis.iter().map(|q| q.value.cooked.as_ref().map(|c| c.to_string()).unwrap_or_else(|| q.value.raw.to_string())).collect()))
        }
        Expression::NumericLiteral(n) => Some(serde_json::json!(n.value)),
        Expression::BooleanLiteral(b) => Some(Value::Bool(b.value)),
        Expression::NullLiteral(_) => Some(Value::Null),
        // `/^@org\/.*/` in ignoreDependencies etc.: keep the source form, matchers parse `/.../`
        Expression::RegExpLiteral(r) => Some(Value::String(format!("/{}/", r.regex.pattern.text))),
        Expression::ArrayExpression(a) => {
            let mut out = Vec::new();
            for el in &a.elements {
                match el {
                    ArrayExpressionElement::SpreadElement(sp) => {
                        if let Some(Value::Array(inner)) = eval(&sp.argument, bindings, depth + 1) {
                            out.extend(inner);
                        }
                    }
                    ArrayExpressionElement::Elision(_) => {}
                    other => {
                        if let Some(x) = other.as_expression() {
                            out.push(eval(x, bindings, depth + 1)?);
                        }
                    }
                }
            }
            Some(Value::Array(out))
        }
        Expression::ObjectExpression(o) => {
            let mut map = Map::new();
            for p in &o.properties {
                match p {
                    ObjectPropertyKind::ObjectProperty(p) => {
                        let key = match &p.key {
                            PropertyKey::StaticIdentifier(i) => i.name.to_string(),
                            PropertyKey::StringLiteral(s) => s.value.to_string(),
                            PropertyKey::NumericLiteral(n) => n.value.to_string(),
                            _ => continue,
                        };
                        if let Some(v) = eval(&p.value, bindings, depth + 1) {
                            map.insert(key, v);
                        }
                    }
                    ObjectPropertyKind::SpreadProperty(sp) => {
                        if let Some(Value::Object(inner)) = eval(&sp.argument, bindings, depth + 1) {
                            map.extend(inner);
                        }
                    }
                }
            }
            Some(Value::Object(map))
        }
        Expression::Identifier(id) => bindings.get(id.name.as_str()).and_then(|e| eval(e, bindings, depth + 1)),
        Expression::TSAsExpression(x) => eval(&x.expression, bindings, depth + 1),
        Expression::TSSatisfiesExpression(x) => eval(&x.expression, bindings, depth + 1),
        Expression::TSNonNullExpression(x) => eval(&x.expression, bindings, depth + 1),
        Expression::TSTypeAssertion(x) => eval(&x.expression, bindings, depth + 1),
        Expression::ParenthesizedExpression(x) => eval(&x.expression, bindings, depth + 1),
        Expression::CallExpression(c) => {
            // defineConfig({...}) and friends: evaluate the first argument
            c.arguments.first().and_then(|a| a.as_expression()).and_then(|a| eval(a, bindings, depth + 1))
        }
        Expression::ArrowFunctionExpression(f) => match &f.body {
            // () => ({...})
            ArrowFunctionBody::FunctionBody(body) => {
                for st in &body.statements {
                    if let Statement::ReturnStatement(r) = st {
                        if let Some(arg) = &r.argument {
                            return eval(arg, bindings, depth + 1);
                        }
                    }
                }
                None
            }
            other => other.as_expression().and_then(|e| eval(e, bindings, depth + 1)),
        },
        _ => None,
    }
}
