// RCL -- A sane configuration language.
// Copyright 2023 Ruud van Asseldonk

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// A copy of the License has been included in the root of the repository.

//! Evaluation turns ASTs into values.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::ast::{BinOp, Expr, Seq, UnOp};
use crate::error::{IntoRuntimeError, Result};
use crate::runtime::{Builtin, Env, Value};
use crate::source::Span;

pub fn eval(env: &mut Env, expr: &Expr) -> Result<Rc<Value>> {
    match expr {
        Expr::BraceLit(seqs) => {
            let mut out = SeqOut::SetOrRecord;
            for seq in seqs {
                eval_seq(env, seq, &mut out)?;
            }
            match out {
                // If we have no keys, it’s a set.
                SeqOut::SetOrRecord => Ok(Rc::new(Value::Set(BTreeSet::new()))),
                SeqOut::Set(_, values) => {
                    let result = values.into_iter().collect();
                    Ok(Rc::new(Value::Set(result)))
                }
                SeqOut::Record(_, keys, values) => {
                    assert_eq!(
                        keys.len(),
                        values.len(),
                        "Should have already reported error about mixing \
                        `k: v` and values in one comprehension."
                    );
                    let mut result = BTreeMap::new();
                    for (k, v) in keys.into_iter().zip(values) {
                        result.insert(k, v);
                    }
                    Ok(Rc::new(Value::Map(result)))
                }
                SeqOut::List(_) => unreachable!("Did not start out as list."),
            }
        }
        Expr::BracketLit(seqs) => {
            let mut out = SeqOut::List(Vec::new());
            for seq in seqs {
                eval_seq(env, seq, &mut out)?;
            }
            match out {
                SeqOut::List(values) => Ok(Rc::new(Value::List(values))),
                _ => unreachable!("SeqOut::List type is preserved."),
            }
        }

        Expr::BoolLit(b) => Ok(Rc::new(Value::Bool(*b))),

        Expr::IntegerLit(i) => Ok(Rc::new(Value::Int(*i))),

        Expr::StringLit(s) => Ok(Rc::new(Value::String(s.clone()))),

        Expr::IfThenElse(if_, then, else_) => {
            let cond = eval(env, if_)?;
            match cond.as_ref() {
                Value::Bool(true) => eval(env, then),
                Value::Bool(false) => eval(env, else_),
                _ => Err("Condition should be boolean.".into()),
            }
        }

        Expr::Var(var) => match env.lookup(var) {
            Some(value) => Ok(value.clone()),
            None => Err("Variable not found.".into()),
        },

        Expr::Field {
            field: field_name,
            inner: inner_expr,
        } => {
            let inner = eval(env, inner_expr)?;
            let field_name_value = Value::String(field_name.0.clone());
            match inner.as_ref() {
                Value::String(s) => {
                    let builtin = match field_name.as_ref() {
                        "len" => Some(builtin_string_len(s)),
                        _ => None,
                    };
                    match builtin {
                        Some(b) => Ok(Rc::new(Value::Builtin(b))),
                        None => Err("No such field in this string.".into()),
                    }
                }
                Value::Map(fields) => {
                    // First test for the builtin names, they shadow the values,
                    // if there are any values.
                    let builtin = match field_name.as_ref() {
                        "contains" => Some(builtin_map_contains(inner.clone())),
                        "get" => Some(builtin_map_get(inner.clone())),
                        _ => None,
                    };
                    if let Some(b) = builtin {
                        return Ok(Rc::new(Value::Builtin(b)));
                    }
                    // If it wasn't a builtin, look for a key in the map.
                    match fields.get(&field_name_value) {
                        Some(v) => Ok(v.clone()),
                        None => {
                            // TODO: Add proper runtime error.
                            // println!("Trying to access {} on:\n{:#?}", field_name, fields);
                            Err("No such field in this value.".into())
                        }
                    }
                }
                Value::List(..) => {
                    let builtin = match field_name.as_ref() {
                        "contains" => Some(builtin_list_contains(inner.clone())),
                        _ => None,
                    };
                    match builtin {
                        Some(b) => Ok(Rc::new(Value::Builtin(b))),
                        None => Err("No such field in this list.".into()),
                    }
                }
                _other => {
                    // TODO: Add proper runtime error.
                    // println!("Trying to access {} on:\n{:#?}", field_name, other);
                    Err("Can only do field access on records for now.".into())
                }
            }
        }

        Expr::Let { ident, value, body } => {
            // Note, this is not a recursive let, the variable is not bound when
            // we evaluate the expression.
            let v_value = eval(env, value)?;
            env.push(ident.clone(), v_value);
            let result = eval(env, body)?;
            env.pop();
            Ok(result)
        }

        Expr::Call {
            function: fun_expr,
            args: args_exprs,
        } => {
            // We do strict evaluation, all arguments get evaluated before we go
            // into the call.
            let fun = eval(env, fun_expr)?;
            let args = args_exprs
                .iter()
                .map(|a| eval(env, a))
                .collect::<Result<Vec<_>>>()?;

            match fun.as_ref() {
                Value::Builtin(f) => (f.f)(&args[..]),
                // TODO: Define a value for lambdas, implement the call.
                _ => Err("Can only call functions.".into()),
            }
        }

        Expr::Lam(_args, _body) => unimplemented!("TODO: Define lambdas."),

        Expr::UnOp(op, value_expr) => {
            let value = eval(env, value_expr)?;
            eval_unop(*op, value)
        }

        Expr::BinOp { op, op_span, lhs: lhs_expr, rhs: rhs_expr } => {
            let lhs = eval(env, lhs_expr)?;
            let rhs = eval(env, rhs_expr)?;
            eval_binop(*op, *op_span, lhs, rhs)
        }
    }
}

fn eval_unop(op: UnOp, v: Rc<Value>) -> Result<Rc<Value>> {
    match (op, v.as_ref()) {
        (UnOp::Neg, Value::Bool(x)) => Ok(Rc::new(Value::Bool(!x))),
        (_op, _val) => {
            // TODO: Add proper runtime error.
            // println!("Trying to apply {:?} to:\n{:#?}", op, val);
            Err("The unary operator is not supported for this value.".into())
        }
    }
}

fn eval_binop(op: BinOp, op_span: Span, lhs: Rc<Value>, rhs: Rc<Value>) -> Result<Rc<Value>> {
    match (op, lhs.as_ref(), rhs.as_ref()) {
        (BinOp::Union, Value::Map(xs), Value::Map(ys)) => {
            let mut result = xs.clone();
            for (k, v) in ys.iter() {
                result.insert(k.clone(), v.clone());
            }
            Ok(Rc::new(Value::Map(result)))
        }
        (BinOp::Union, Value::Set(xs), Value::Set(ys)) => {
            let result = xs.union(ys).cloned().collect();
            Ok(Rc::new(Value::Set(result)))
        }
        (BinOp::Union, Value::Set(xs), Value::List(ys)) => {
            let mut result = xs.clone();
            result.extend(ys.iter().cloned());
            Ok(Rc::new(Value::Set(result)))
        }
        // TODO: Could evaluate these boolean expressions lazily, if the
        // language is really pure. But if I enable external side effects like
        // running a program to read its input, that would be questionable to do.
        (BinOp::And, Value::Bool(x), Value::Bool(y)) => Ok(Rc::new(Value::Bool(*x && *y))),
        (BinOp::Or, Value::Bool(x), Value::Bool(y)) => Ok(Rc::new(Value::Bool(*x || *y))),
        (BinOp::Add, Value::Int(x), Value::Int(y)) => {
            match x.checked_add(*y) {
                Some(z) => Ok(Rc::new(Value::Int(z))),
                // TODO: Also include the values themselves through pretty-printer.
                None => return Err(op_span.error("Addition would overflow.").into()),
            }
        }
        (BinOp::Mul, Value::Int(x), Value::Int(y)) => {
            match x.checked_mul(*y) {
                Some(z) => Ok(Rc::new(Value::Int(z))),
                // TODO: Also include the values themselves through pretty-printer.
                None => return Err(op_span.error("Multiplication would overflow.").into()),
            }
        }
        (BinOp::Lt, Value::Int(x), Value::Int(y)) => Ok(Rc::new(Value::Bool(*x < *y))),
        (BinOp::Gt, Value::Int(x), Value::Int(y)) => Ok(Rc::new(Value::Bool(*x > *y))),
        (BinOp::LtEq, Value::Int(x), Value::Int(y)) => Ok(Rc::new(Value::Bool(*x <= *y))),
        (BinOp::GtEq, Value::Int(x), Value::Int(y)) => Ok(Rc::new(Value::Bool(*x >= *y))),
        _ => {
            // TODO: Add a proper runtime error.
            // println!("Trying to apply {:?} to:\n{:#?}\n{:#?}", op, lhs, rhs);
            Err("The binary operator is not supported for this value.".into())
        }
    }
}

/// Evaluation output for sequences.
enum SeqOut {
    /// Only allow scalar values because we are in a list literal.
    List(Vec<Rc<Value>>),

    /// The sequence is definitely a set, because the first element is scalar.
    ///
    /// The span contains the previous scalar as evidence.
    Set(Span, Vec<Rc<Value>>),

    /// The sequence is definitely a record, because the first element is kv.
    ///
    /// The span contains the previous key-value as evidence.
    Record(Span, Vec<Rc<Value>>, Vec<Rc<Value>>),

    /// It's still unclear whether this is a set or a record.
    SetOrRecord,
}

impl SeqOut {
    fn values(&mut self, scalar: Span) -> Result<&mut Vec<Rc<Value>>> {
        match self {
            SeqOut::SetOrRecord => {
                *self = SeqOut::Set(scalar, Vec::new());
                self.values(scalar)
            }
            SeqOut::List(values) => Ok(values),
            SeqOut::Set(_first, values) => Ok(values),
            SeqOut::Record(first, _keys, _values) => {
                let err = scalar
                    .error("Expected key-value, not a scalar element.")
                    .with_note(
                        *first,
                        "The collection is a record and not a set, because of this key-value.",
                    );
                return Err(err.into());
            }
        }
    }

    fn keys_values(&mut self, kv: Span) -> Result<(&mut Vec<Rc<Value>>, &mut Vec<Rc<Value>>)> {
        match self {
            SeqOut::SetOrRecord => {
                *self = SeqOut::Record(kv, Vec::new(), Vec::new());
                self.keys_values(kv)
            }
            SeqOut::List(_values) => {
                let err = kv
                    .error("Expected scalar element, not key-value.")
                    .with_help(
                    "Key-value pairs are allowed in records, which are enclosed in '{}', not '[]'.",
                );
                return Err(err.into());
            }
            SeqOut::Set(first, _values) => {
                let err = kv
                    .error("Expected scalar element, not key-value.")
                    .with_note(
                        *first,
                        "The collection is a set and not a record, because of this scalar value.",
                    );
                return Err(err.into());
            }
            SeqOut::Record(_first, keys, values) => Ok((keys, values)),
        }
    }
}

fn eval_seq(env: &mut Env, seq: &Seq, out: &mut SeqOut) -> Result<()> {
    match seq {
        Seq::Elem {
            span,
            value: value_expr,
        } => {
            let out_values = out.values(*span)?;
            let value = eval(env, value_expr)?;
            out_values.push(value);
            Ok(())
        }
        Seq::Assoc {
            op_span,
            key: key_expr,
            value: value_expr,
        } => {
            let (out_keys, out_values) = out.keys_values(*op_span)?;
            let key = eval(env, key_expr)?;
            let value = eval(env, value_expr)?;
            out_keys.push(key);
            out_values.push(value);
            Ok(())
        }
        Seq::For {
            idents,
            collection,
            body,
        } => {
            let collection_value = eval(env, collection)?;
            match (&idents[..], collection_value.as_ref()) {
                (&[ref name], Value::List(xs)) => {
                    for x in xs {
                        env.push(name.clone(), x.clone());
                        eval_seq(env, body, out)?;
                        env.pop();
                    }
                    Ok(())
                }
                (&[ref name], Value::Set(xs)) => {
                    for x in xs {
                        env.push(name.clone(), x.clone());
                        eval_seq(env, body, out)?;
                        env.pop();
                    }
                    Ok(())
                }
                (&[ref k_name, ref v_name], Value::Map(xs)) => {
                    for (k, v) in xs {
                        env.push(k_name.clone(), k.clone());
                        env.push(v_name.clone(), v.clone());
                        eval_seq(env, body, out)?;
                        env.pop();
                        env.pop();
                    }
                    Ok(())
                }
                _ => Err("Iteration is not supported like this.".into()),
            }
        }
        Seq::If { condition, body } => {
            let cond = eval(env, condition)?;
            match cond.as_ref() {
                Value::Bool(true) => eval_seq(env, body, out),
                Value::Bool(false) => Ok(()),
                _ => Err("Comprehension condition should be boolean.".into()),
            }
        }
        Seq::Let { ident, value, body } => {
            let v = eval(env, value)?;
            env.push(ident.clone(), v);
            eval_seq(env, body, out)?;
            env.pop();
            Ok(())
        }
    }
}

fn builtin_string_len(s: &str) -> Builtin {
    let n = Rc::new(Value::Int(s.len() as _));
    let f = move |args: &[Rc<Value>]| {
        if args.len() > 0 {
            return Err("String.len takes no arguments.".into());
        };
        Ok(n.clone())
    };
    Builtin {
        name: "String.len",
        f: Box::new(f),
    }
}

fn builtin_map_contains(v: Rc<Value>) -> Builtin {
    let f = move |args: &[Rc<Value>]| {
        let arg = match args {
            [a] => a,
            _ => return Err("Map.contains takes a single argument.".into()),
        };
        match v.as_ref() {
            Value::Map(m) => {
                let contains = m.contains_key(arg);
                Ok(Rc::new(Value::Bool(contains)))
            }
            _not_map => panic!("Should not have made a Map.contains for this value."),
        }
    };
    Builtin {
        name: "Map.contains",
        f: Box::new(f),
    }
}

fn builtin_list_contains(v: Rc<Value>) -> Builtin {
    let f = move |args: &[Rc<Value>]| {
        let arg = match args {
            [a] => a,
            _ => return Err("List.contains takes a single argument.".into()),
        };
        match v.as_ref() {
            Value::List(m) => {
                let contains = m.contains(arg);
                Ok(Rc::new(Value::Bool(contains)))
            }
            _not_map => panic!("Should not have made a List.contains for this value."),
        }
    };
    Builtin {
        name: "List.contains",
        f: Box::new(f),
    }
}

fn builtin_map_get(v: Rc<Value>) -> Builtin {
    let f = move |args: &[Rc<Value>]| {
        let (k, default) = match args {
            [k, default] => (k, default),
            _ => return Err("Map.get takes two arguments.".into()),
        };
        match v.as_ref() {
            Value::Map(m) => match m.get(k) {
                Some(v) => Ok(v.clone()),
                None => Ok(default.clone()),
            },
            _not_map => panic!("Should not have made a Map.get for this value."),
        }
    };
    Builtin {
        name: "Map.get",
        f: Box::new(f),
    }
}
