// RCL -- A reasonable configuration language.
// Copyright 2024 Ruud van Asseldonk

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// A copy of the License has been included in the root of the repository.

// The names of the rules here, and the general structure, are modelled after
// the simpler Bison grammar in //grammar/bison/grammar.y.

const binops = {
  and: "and",
  or: "or",
  pipe: "|",
  star: "*",
  plus: "+",
  minus: "-",
  slash: "/",
  lt: "<",
  lt_eq: "<=",
  gt: ">",
  gt_eq: ">=",
  eq: "==",
  neq: "!=",
};

module.exports = grammar({
  name: "rcl",

  word: $ => $.ident,

  rules: {
    source_file: $ => seq(repeat($._prefix), $._expr),

    // A blank is whitespace with two or more newlines. This token is distinct
    // from the regular whitespace, that Tree-sitter by default allows anywhere.
    blank: $ => /[ \t\r\f]*\n[ \t\r\f]*\n[ \t\r\n\f]*/,
    comment: $ => /\/\/[^\n]*\n/,
    _prefix: $ => choice($.blank, $.comment),

    ident: $ => /[_A-Za-z][-_A-Za-z0-9]*/,

    // TODO: Implement the custom lexer to handle string literals.
    string: $ => /"[^"]*"/,

    number: $ => choice($.num_binary, $.num_hexadecimal, $.num_decimal),
    num_binary: $ => /0b[01_]*/,
    num_hexadecimal: $ => /0x[0-9a-fA-F_]*/,
    num_decimal: $ => /(0|[1-9][0-9_]*)(\.[0-9][0-9_]*)?([eE][-+]?[0-9][0-9_]*)?/,

    unop: $ => choice("not", "-"),
    binop: $ => choice(...Object.values(binops)),

    _expr: $ => choice(
      $.expr_stmt,
      $._expr_op,
    ),
    expr_stmt: $ => seq($._stmt, ";", repeat($._prefix), $._expr),

    _expr_op: $ => choice(
      $.expr_unop,
      $._expr_not_op,
      ...Object.keys(binops).map(op => $[`expr_binop_${op}`]),
    ),

    expr_unop: $ => choice(
      seq($.unop, $._expr_not_op),
      seq($.unop, $.expr_unop),
    ),

    // Because RCL does not have operator precedence, every binary operator
    // gets its own rule in the grammar.
    ...Object.fromEntries(Object.entries(binops).map(name_op => [
      `expr_binop_${name_op[0]}`,
      $ => seq(
        $._expr_not_op,
        repeat1(seq(field("binop", name_op[1]), $._expr_not_op)),
      ),
    ])),

    _expr_not_op: $ => choice(
      $._expr_term,
      $.expr_call,
      $.expr_index,
      $.expr_field,
    ),
    expr_call: $ => seq(
      field("function", $._expr_not_op),
      "(",
      field("args", optional($._call_args)),
      ")",
    ),
    expr_index: $ => seq(
      field("collection", $._expr_not_op),
      "[",
      field("index", $._expr),
      "]",
    ),
    expr_field: $ => seq(
      field("inner", $._expr_not_op),
      ".",
      field("field", $.ident),
    ),

    _call_args: $ => choice(
      seq(repeat($._prefix), $._expr),
      seq(repeat($._prefix), $._expr, ",", choice(optional($._call_args), repeat($._prefix))),
    ),

    _expr_term: $ => choice(
      $.expr_term_braces,
      $.expr_term_brackets,
      $.expr_term_parens,
      $.string,
      $.number,
      $.ident,
    ),
    expr_term_braces:   $ => seq("{", optional($._seqs), "}"),
    expr_term_brackets: $ => seq("[", optional($._seqs), "]"),
    expr_term_parens:   $ => seq("(", optional($._seqs), ")"),

    _stmt: $ => seq(choice($.stmt_let)),
    stmt_let: $ => seq(
      "let",
      field("ident", $.ident),
      "=",
      repeat($._prefix),
      field("value", $._expr),
    ),

    // One or more `seq`s with an optional trailing comma. The use site has to
    // wrap it in `optional` as Tree-sitter does not support rules that match
    // the empty string.
    _seqs: $ => choice(
      seq(repeat($._prefix), $._seq),
      seq(repeat($._prefix), $._seq, ",", choice(optional($._seqs), repeat($._prefix))),
    ),

    _seq: $ => choice(
      $.seq_elem,
      $.seq_assoc_expr,
      $.seq_assoc_ident,
      $.seq_stmt,
      $.seq_for,
      $.seq_if,
    ),
    seq_elem: $ => $._expr_op,
    seq_assoc_expr: $ => seq(
      field("field", $._expr_op),
      ":",
      field("value", $._expr),
    ),
    seq_assoc_ident: $ => seq(
      field("field", $.ident),
      "=",
      field("value", $._expr),
    ),
    // TODO: I need to allow a prefix here. Will the fuzzer find it?
    seq_stmt: $ => seq($._stmt, ";", $._seq),
    seq_for: $ => seq(
      "for",
      field("idents", $._idents),
      "in",
      field("collection", $._expr),
      ":",
      field("body", $._seq),
    ),
    seq_if: $ => seq(
      "if",
      field("condition", $._expr),
      ":",
      field("body", $._seq),
    ),

    // One or more identifiers separated by comma, no trailing comma allowed.
    _idents: $=> seq($.ident, repeat(seq(",", $.ident))),
  }
});
