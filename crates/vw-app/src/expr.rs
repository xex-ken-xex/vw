//! 式ノードの式。画素ごとに評価するので、一度だけ構文を読んで命令の列（スタック機械）にし、
//! 画素ごとはその列をなぞるだけにする。
//!
//! 式のクレート（evalexpr など）を使わないのは、画素ごとに木をたどる評価が遅く、
//! 2000 万画素で数秒かかるため。文法は四則・比較・三項演算と少しの関数だけで足りる。

use std::fmt;

/// 式の中で使える変数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Env {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// 今書き込もうとしているチャンネルの値。
    pub v: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Var {
    R,
    G,
    B,
    A,
    V,
    X,
    Y,
    W,
    H,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Abs,
    Sqrt,
    Exp,
    Log,
    Sin,
    Cos,
    Tan,
    Floor,
    Ceil,
    Round,
    Fract,
    Min,
    Max,
    Pow,
    Step,
    Mix,
    Clamp,
    Smoothstep,
}

impl Func {
    const ALL: [(&'static str, Func, usize); 18] = [
        ("abs", Func::Abs, 1),
        ("sqrt", Func::Sqrt, 1),
        ("exp", Func::Exp, 1),
        ("log", Func::Log, 1),
        ("sin", Func::Sin, 1),
        ("cos", Func::Cos, 1),
        ("tan", Func::Tan, 1),
        ("floor", Func::Floor, 1),
        ("ceil", Func::Ceil, 1),
        ("round", Func::Round, 1),
        ("fract", Func::Fract, 1),
        ("min", Func::Min, 2),
        ("max", Func::Max, 2),
        ("pow", Func::Pow, 2),
        ("step", Func::Step, 2),
        ("mix", Func::Mix, 3),
        ("clamp", Func::Clamp, 3),
        ("smoothstep", Func::Smoothstep, 3),
    ];

    fn by_name(name: &str) -> Option<(Func, usize)> {
        Self::ALL
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, f, arity)| (*f, *arity))
    }
}

/// 式に書ける関数の名前（ヘルプ用）。
pub fn function_names() -> impl Iterator<Item = &'static str> {
    Func::ALL.iter().map(|(n, _, _)| *n)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Const(f32),
    Var(Var),
    Neg,
    Not,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    /// 条件・真のときの値・偽のときの値の 3 つを取り、どちらかを残す。
    Select,
    Call(Func),
}

/// 1 行がどのチャンネルに書き込むか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// すべてのチャンネル（`v` がそのチャンネルの値）。
    All,
    R,
    G,
    B,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    /// 1 から数えた行。
    pub line: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// 読み終えた式。行ごとに 1 つの出力。
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    lines: Vec<(Target, Vec<Op>)>,
    /// 評価に要るスタックの深さ。
    depth: usize,
}

impl Program {
    /// `text` の各行を読む。空行と `#` で始まる行は飛ばす。`r:` `g:` `b:` `v:` で始まる行は
    /// そのチャンネルへ、それ以外の行はすべてのチャンネルへ書く。
    pub fn compile(text: &str) -> Result<Self, Error> {
        let mut lines = Vec::new();
        let mut depth = 0;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (target, body) = match line.split_once(':') {
                Some((head, rest)) if !head.contains('?') => {
                    let target = match head.trim() {
                        "r" => Target::R,
                        "g" => Target::G,
                        "b" => Target::B,
                        "v" => Target::All,
                        other => {
                            return Err(Error {
                                line: i + 1,
                                message: format!("unknown channel \"{other}\" (use r, g, b or v)"),
                            })
                        }
                    };
                    (target, rest)
                }
                _ => (Target::All, line),
            };
            let ops = Parser::new(body)
                .and_then(|mut p| p.program())
                .map_err(|message| Error {
                    line: i + 1,
                    message,
                })?;
            depth = depth.max(stack_depth(&ops));
            lines.push((target, ops));
        }
        if lines.is_empty() {
            return Err(Error {
                line: 1,
                message: "the expression is empty".into(),
            });
        }
        Ok(Self { lines, depth })
    }

    /// `env` で各行を評価する。戻り値は行ごとの (出力先, 値)。
    pub fn eval_into(&self, env: &Env, stack: &mut Vec<f32>, out: &mut Vec<(Target, f32)>) {
        out.clear();
        for (target, ops) in &self.lines {
            let env = Env {
                v: match target {
                    Target::All => env.v,
                    Target::R => env.r,
                    Target::G => env.g,
                    Target::B => env.b,
                },
                ..*env
            };
            out.push((*target, run(ops, &env, stack)));
        }
    }

    /// スタックを用意して 1 回だけ評価する。
    #[cfg(test)]
    pub fn eval(&self, env: &Env) -> Vec<(Target, f32)> {
        let mut stack = Vec::with_capacity(self.depth);
        let mut out = Vec::new();
        self.eval_into(env, &mut stack, &mut out);
        out
    }
}

fn run(ops: &[Op], env: &Env, stack: &mut Vec<f32>) -> f32 {
    stack.clear();
    for op in ops {
        let v = match *op {
            Op::Const(c) => c,
            Op::Var(var) => match var {
                Var::R => env.r,
                Var::G => env.g,
                Var::B => env.b,
                Var::A => env.a,
                Var::V => env.v,
                Var::X => env.x,
                Var::Y => env.y,
                Var::W => env.w,
                Var::H => env.h,
            },
            Op::Neg => -pop(stack),
            Op::Not => bool_of(pop(stack) == 0.0),
            Op::Select => {
                let no = pop(stack);
                let yes = pop(stack);
                let cond = pop(stack);
                if cond != 0.0 {
                    yes
                } else {
                    no
                }
            }
            Op::Call(f) => call(f, stack),
            binary => {
                let b = pop(stack);
                let a = pop(stack);
                match binary {
                    Op::Add => a + b,
                    Op::Sub => a - b,
                    Op::Mul => a * b,
                    Op::Div => a / b,
                    Op::Rem => a % b,
                    Op::Pow => a.powf(b),
                    Op::Lt => bool_of(a < b),
                    Op::Le => bool_of(a <= b),
                    Op::Gt => bool_of(a > b),
                    Op::Ge => bool_of(a >= b),
                    Op::Eq => bool_of(a == b),
                    Op::Ne => bool_of(a != b),
                    Op::And => bool_of(a != 0.0 && b != 0.0),
                    Op::Or => bool_of(a != 0.0 || b != 0.0),
                    _ => unreachable!(),
                }
            }
        };
        stack.push(v);
    }
    pop(stack)
}

fn pop(stack: &mut Vec<f32>) -> f32 {
    stack.pop().unwrap_or(0.0)
}

fn bool_of(b: bool) -> f32 {
    if b {
        1.0
    } else {
        0.0
    }
}

fn call(f: Func, stack: &mut Vec<f32>) -> f32 {
    match f {
        Func::Abs => pop(stack).abs(),
        Func::Sqrt => pop(stack).sqrt(),
        Func::Exp => pop(stack).exp(),
        Func::Log => pop(stack).ln(),
        Func::Sin => pop(stack).sin(),
        Func::Cos => pop(stack).cos(),
        Func::Tan => pop(stack).tan(),
        Func::Floor => pop(stack).floor(),
        Func::Ceil => pop(stack).ceil(),
        Func::Round => pop(stack).round(),
        Func::Fract => pop(stack).fract(),
        Func::Min | Func::Max | Func::Pow | Func::Step => {
            let b = pop(stack);
            let a = pop(stack);
            match f {
                Func::Min => a.min(b),
                Func::Max => a.max(b),
                Func::Pow => a.powf(b),
                // GLSL と同じ step(edge, x)。
                _ => bool_of(b >= a),
            }
        }
        Func::Mix | Func::Clamp | Func::Smoothstep => {
            let c = pop(stack);
            let b = pop(stack);
            let a = pop(stack);
            match f {
                Func::Mix => a + (b - a) * c,
                Func::Clamp => a.clamp(b.min(c), c.max(b)),
                _ => {
                    let t = ((c - a) / (b - a)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                }
            }
        }
    }
}

/// 命令の列が使うスタックの最大の深さ。
fn stack_depth(ops: &[Op]) -> usize {
    let (mut now, mut max) = (0usize, 0usize);
    for op in ops {
        let (takes, gives) = match op {
            Op::Const(_) | Op::Var(_) => (0, 1),
            Op::Neg | Op::Not => (1, 1),
            Op::Select => (3, 1),
            Op::Call(f) => (Func::ALL.iter().find(|(_, g, _)| g == f).unwrap().2, 1),
            _ => (2, 1),
        };
        now = now.saturating_sub(takes) + gives;
        max = max.max(now);
    }
    max
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f32),
    Ident(String),
    Punct(&'static str),
}

const PUNCTS: [&str; 20] = [
    "<=", ">=", "==", "!=", "&&", "||", "+", "-", "*", "/", "%", "^", "<", ">", "!", "?", ":", "(",
    ")", ",",
];

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let c = rest.chars().next().unwrap();
        if c.is_ascii_digit() || (c == '.' && rest[1..].starts_with(|d: char| d.is_ascii_digit())) {
            let end = rest
                .find(|d: char| !(d.is_ascii_digit() || d == '.' || d == 'e' || d == 'E'))
                .unwrap_or(rest.len());
            // 1e-3 の「-」も数の一部。
            let end = if rest[..end].ends_with(['e', 'E']) && rest[end..].starts_with(['-', '+']) {
                end + 1
                    + rest[end + 1..]
                        .find(|d: char| !d.is_ascii_digit())
                        .unwrap_or(rest.len() - end - 1)
            } else {
                end
            };
            let number = rest[..end]
                .parse::<f32>()
                .map_err(|_| format!("bad number \"{}\"", &rest[..end]))?;
            tokens.push(Token::Number(number));
            rest = &rest[end..];
        } else if c.is_ascii_alphabetic() || c == '_' {
            let end = rest
                .find(|d: char| !(d.is_ascii_alphanumeric() || d == '_'))
                .unwrap_or(rest.len());
            tokens.push(Token::Ident(rest[..end].to_owned()));
            rest = &rest[end..];
        } else if let Some(p) = PUNCTS.iter().find(|p| rest.starts_with(*p)) {
            tokens.push(Token::Punct(p));
            rest = &rest[p.len()..];
        } else {
            return Err(format!("unexpected character \"{c}\""));
        }
        rest = rest.trim_start();
    }
    Ok(tokens)
}

/// 再帰下降で読み、読みながら命令を積む。
struct Parser {
    tokens: Vec<Token>,
    at: usize,
    ops: Vec<Op>,
}

impl Parser {
    fn new(text: &str) -> Result<Self, String> {
        Ok(Self {
            tokens: tokenize(text)?,
            at: 0,
            ops: Vec::new(),
        })
    }

    fn program(&mut self) -> Result<Vec<Op>, String> {
        if self.tokens.is_empty() {
            return Err("the expression is empty".into());
        }
        self.ternary()?;
        if let Some(t) = self.tokens.get(self.at) {
            return Err(format!("unexpected {}", describe(t)));
        }
        Ok(std::mem::take(&mut self.ops))
    }

    fn peek(&self, p: &str) -> bool {
        matches!(self.tokens.get(self.at), Some(Token::Punct(q)) if *q == p)
    }

    fn eat(&mut self, p: &str) -> bool {
        let found = self.peek(p);
        if found {
            self.at += 1;
        }
        found
    }

    fn expect(&mut self, p: &str) -> Result<(), String> {
        if self.eat(p) {
            Ok(())
        } else {
            Err(match self.tokens.get(self.at) {
                Some(t) => format!("expected \"{p}\" but found {}", describe(t)),
                None => format!("expected \"{p}\" at the end"),
            })
        }
    }

    fn ternary(&mut self) -> Result<(), String> {
        self.or()?;
        if self.eat("?") {
            self.ternary()?;
            self.expect(":")?;
            self.ternary()?;
            self.ops.push(Op::Select);
        }
        Ok(())
    }

    fn or(&mut self) -> Result<(), String> {
        self.and()?;
        while self.eat("||") {
            self.and()?;
            self.ops.push(Op::Or);
        }
        Ok(())
    }

    fn and(&mut self) -> Result<(), String> {
        self.comparison()?;
        while self.eat("&&") {
            self.comparison()?;
            self.ops.push(Op::And);
        }
        Ok(())
    }

    fn comparison(&mut self) -> Result<(), String> {
        self.additive()?;
        let ops = [
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("==", Op::Eq),
            ("!=", Op::Ne),
            ("<", Op::Lt),
            (">", Op::Gt),
        ];
        if let Some((_, op)) = ops.iter().find(|(p, _)| self.peek(p)) {
            self.at += 1;
            self.additive()?;
            self.ops.push(*op);
        }
        Ok(())
    }

    fn additive(&mut self) -> Result<(), String> {
        self.multiplicative()?;
        loop {
            let op = if self.eat("+") {
                Op::Add
            } else if self.eat("-") {
                Op::Sub
            } else {
                return Ok(());
            };
            self.multiplicative()?;
            self.ops.push(op);
        }
    }

    fn multiplicative(&mut self) -> Result<(), String> {
        self.unary()?;
        loop {
            let op = if self.eat("*") {
                Op::Mul
            } else if self.eat("/") {
                Op::Div
            } else if self.eat("%") {
                Op::Rem
            } else {
                return Ok(());
            };
            self.unary()?;
            self.ops.push(op);
        }
    }

    fn unary(&mut self) -> Result<(), String> {
        if self.eat("-") {
            self.unary()?;
            self.ops.push(Op::Neg);
        } else if self.eat("!") {
            self.unary()?;
            self.ops.push(Op::Not);
        } else {
            self.power()?;
        }
        Ok(())
    }

    fn power(&mut self) -> Result<(), String> {
        self.atom()?;
        if self.eat("^") {
            // 右結合: 2^3^2 = 2^9。
            self.unary()?;
            self.ops.push(Op::Pow);
        }
        Ok(())
    }

    fn atom(&mut self) -> Result<(), String> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        match token {
            Some(Token::Number(n)) => self.ops.push(Op::Const(n)),
            Some(Token::Punct("(")) => {
                self.ternary()?;
                self.expect(")")?;
            }
            Some(Token::Ident(name)) => {
                if self.eat("(") {
                    let (f, arity) = Func::by_name(&name)
                        .ok_or_else(|| format!("unknown function \"{name}\""))?;
                    let mut n = 0;
                    if !self.peek(")") {
                        loop {
                            self.ternary()?;
                            n += 1;
                            if !self.eat(",") {
                                break;
                            }
                        }
                    }
                    self.expect(")")?;
                    if n != arity {
                        return Err(format!(
                            "{name} takes {arity} argument{} but got {n}",
                            if arity == 1 { "" } else { "s" }
                        ));
                    }
                    self.ops.push(Op::Call(f));
                } else {
                    let op = match name.as_str() {
                        "r" => Op::Var(Var::R),
                        "g" => Op::Var(Var::G),
                        "b" => Op::Var(Var::B),
                        "a" => Op::Var(Var::A),
                        "v" => Op::Var(Var::V),
                        "x" => Op::Var(Var::X),
                        "y" => Op::Var(Var::Y),
                        "w" => Op::Var(Var::W),
                        "h" => Op::Var(Var::H),
                        "pi" => Op::Const(std::f32::consts::PI),
                        "e" => Op::Const(std::f32::consts::E),
                        _ => return Err(format!("unknown name \"{name}\"")),
                    };
                    self.ops.push(op);
                }
            }
            Some(t) => return Err(format!("unexpected {}", describe(&t))),
            None => return Err("the expression ends too early".into()),
        }
        Ok(())
    }
}

fn describe(t: &Token) -> String {
    match t {
        Token::Number(n) => format!("number {n}"),
        Token::Ident(s) => format!("\"{s}\""),
        Token::Punct(p) => format!("\"{p}\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(v: f32) -> Env {
        Env {
            r: 0.25,
            g: 0.5,
            b: 0.75,
            a: 1.0,
            v,
            x: 3.0,
            y: 4.0,
            w: 10.0,
            h: 20.0,
        }
    }

    fn one(text: &str, v: f32) -> f32 {
        Program::compile(text).unwrap().eval(&env(v))[0].1
    }

    #[test]
    fn arithmetic_follows_the_usual_precedence() {
        assert_eq!(one("1 + 2 * 3", 0.0), 7.0);
        assert_eq!(one("(1 + 2) * 3", 0.0), 9.0);
        assert_eq!(one("2 ^ 3 ^ 2", 0.0), 512.0);
        assert_eq!(one("-2 ^ 2", 0.0), -4.0);
        assert_eq!(one("7 % 4 / 2", 0.0), 1.5);
        assert_eq!(one("1e-1 + .5", 0.0), 0.6);
    }

    #[test]
    fn variables_and_functions_read_the_pixel() {
        assert_eq!(one("v * 2", 0.3), 0.6);
        assert_eq!(one("r + g + b + a", 0.0), 2.5);
        assert_eq!(one("x / w + y / h", 0.0), 0.5);
        assert_eq!(one("clamp(v, 0, 1)", 1.5), 1.0);
        assert_eq!(one("mix(0, 10, 0.25)", 0.0), 2.5);
        assert_eq!(one("step(0.5, v)", 0.7), 1.0);
        assert_eq!(one("smoothstep(0, 1, 0.5)", 0.0), 0.5);
        assert!((one("pow(v, 1 / 2.2)", 0.5) - 0.7297).abs() < 1e-3);
        assert_eq!(one("abs(-pi) > 3", 0.0), 1.0);
    }

    #[test]
    fn comparisons_and_ternary_select_a_branch() {
        assert_eq!(one("v > 0.5 ? 1 : 0", 0.7), 1.0);
        assert_eq!(one("v > 0.5 ? 1 : 0", 0.2), 0.0);
        assert_eq!(one("v >= 0.2 && v <= 0.8 ? v : 0", 0.9), 0.0);
        assert_eq!(one("!(v == 1) || 0", 1.0), 0.0);
        assert_eq!(one("x < 5 ? y < 5 ? 1 : 2 : 3", 0.0), 1.0);
    }

    #[test]
    fn channel_lines_write_their_own_channel() {
        let p = Program::compile("# swap\nr: b\nb: r\ng: g * 0").unwrap();
        let out = p.eval(&env(0.0));
        assert_eq!(
            out,
            vec![(Target::R, 0.75), (Target::B, 0.25), (Target::G, 0.0)]
        );
        let p = Program::compile("v: 1 - v").unwrap();
        assert_eq!(p.eval(&env(0.3)), vec![(Target::All, 0.7)]);
    }

    #[test]
    fn errors_name_the_line_and_the_problem() {
        let e = Program::compile("v * 2\nfoo(v)").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.message.contains("foo"), "{e}");
        assert!(Program::compile("min(v)")
            .unwrap_err()
            .message
            .contains("2 argument"));
        assert!(Program::compile("(v")
            .unwrap_err()
            .message
            .contains("\")\""));
        assert!(Program::compile("v $ 2").unwrap_err().message.contains("$"));
        assert!(Program::compile("q: v")
            .unwrap_err()
            .message
            .contains("channel"));
        assert!(Program::compile("\n# nothing\n").is_err());
        assert!(Program::compile("v 2")
            .unwrap_err()
            .message
            .contains("unexpected"));
    }

    #[test]
    fn division_by_zero_and_missing_operands_do_not_panic() {
        assert!(one("1 / 0", 0.0).is_infinite());
        assert!(one("log(-1)", 0.0).is_nan());
    }
}
