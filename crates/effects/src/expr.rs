//! The numbers an `effect.toml` works out from its params: how far an effect
//! reads past its part (`reach`), how far it draws past it (`bleed`), how many
//! times a stage runs (`repeat`), whether it runs at all (`when`), and the
//! params it hands an effect it `use`s.
//!
//! A small language, on purpose: numbers, param names, `+ - * / ^`, the
//! comparisons `< <= > >= == ~=` (`!=` too), `and or not`, parentheses, and
//! `min max clamp floor ceil abs sqrt`. No loops, no strings, no calls of
//! anything else, so an expression is parsed once when its effect is loaded
//! and evaluated in microseconds whenever its params are bound, by the
//! compositor alone: it plans every rung of a `fallback` itself. Precedence
//! is Lua's, which the `effect.lua` these replace was written in: `or`, then
//! `and`, the comparisons, `+ -`, `* /`, the unary `not` and `-`, and `^`,
//! which groups to the right and binds tighter than a minus before it.
//!
//! Every value is a number. A comparison is 1 or 0, and `and`, `or`, `not`
//! and a stage's `when` take anything but 0 as true. A param that is `true` or
//! `false` is 1 or 0. Evaluation is total: nothing panics, and a division by
//! 0 gives an infinity or NaN, which whatever reads the value refuses.

use crate::spec::{Value, nearest};

/// The longest expression read, in bytes, and the deepest nesting of
/// parentheses and operators in one: an expression is a line of a file, and
/// a parser that recursed as deep as a file asked could overflow the
/// compositor's stack (`tests::nesting_past_the_limit_is_refused_not_followed`).
const MOST_BYTES: usize = 1024;
const MOST_DEPTH: usize = 64;

/// The functions an expression may call, and how many arguments each takes:
/// at least, and at most (`None`: any number more).
const FUNCTIONS: [(&str, usize, Option<usize>); 7] = [
    ("min", 1, None),
    ("max", 1, None),
    ("clamp", 3, Some(3)),
    ("floor", 1, Some(1)),
    ("ceil", 1, Some(1)),
    ("abs", 1, Some(1)),
    ("sqrt", 1, Some(1)),
];

/// An expression, parsed: what it says, and the text it was read from.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    node: Node,
    text: String,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Number(f64),
    Param(String),
    Negate(Box<Node>),
    Not(Box<Node>),
    Binary(Op, Box<Node>, Box<Node>),
    Call(usize, Vec<Node>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Or,
    And,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
}

impl Op {
    /// How tightly it binds: higher first. `^` is the only one that groups
    /// to the right.
    const fn rank(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Less
            | Self::LessEqual
            | Self::Greater
            | Self::GreaterEqual
            | Self::Equal
            | Self::NotEqual => 3,
            Self::Add | Self::Subtract => 4,
            Self::Multiply | Self::Divide => 5,
            Self::Power => 7,
        }
    }
}

/// The rank of the unary `not` and `-`: above `*`, below `^`.
const UNARY: u8 = 6;

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Op(Op),
    Not,
    Minus,
    Open,
    Close,
    Comma,
}

/// One token and the byte it starts at, for a problem to point at.
type Placed = (Token, usize);

fn tokens(text: &str) -> Result<Vec<Placed>, String> {
    let bytes = text.as_bytes();
    let mut read = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if byte.is_ascii_digit() || byte == b'.' {
            while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'.') {
                at += 1;
            }
            if at < bytes.len() && (bytes[at] == b'e' || bytes[at] == b'E') {
                at += 1;
                if at < bytes.len() && (bytes[at] == b'+' || bytes[at] == b'-') {
                    at += 1;
                }
                while at < bytes.len() && bytes[at].is_ascii_digit() {
                    at += 1;
                }
            }
            let number = text[start..at]
                .parse::<f64>()
                .map_err(|_| format!("`{}` at {start} is not a number", &text[start..at]))?;
            read.push((Token::Number(number), start));
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            let word = &text[start..at];
            let token = match word {
                "and" => Token::Op(Op::And),
                "or" => Token::Op(Op::Or),
                "not" => Token::Not,
                "true" => Token::Number(1.0),
                "false" => Token::Number(0.0),
                _ => Token::Name(word.to_owned()),
            };
            read.push((token, start));
            continue;
        }
        let next = bytes.get(at + 1).copied();
        let (token, width) = match (byte, next) {
            (b'<', Some(b'=')) => (Token::Op(Op::LessEqual), 2),
            (b'>', Some(b'=')) => (Token::Op(Op::GreaterEqual), 2),
            (b'=', Some(b'=')) => (Token::Op(Op::Equal), 2),
            (b'~' | b'!', Some(b'=')) => (Token::Op(Op::NotEqual), 2),
            (b'<', _) => (Token::Op(Op::Less), 1),
            (b'>', _) => (Token::Op(Op::Greater), 1),
            (b'+', _) => (Token::Op(Op::Add), 1),
            (b'-', _) => (Token::Minus, 1),
            (b'*', _) => (Token::Op(Op::Multiply), 1),
            (b'/', _) => (Token::Op(Op::Divide), 1),
            (b'^', _) => (Token::Op(Op::Power), 1),
            (b'(', _) => (Token::Open, 1),
            (b')', _) => (Token::Close, 1),
            (b',', _) => (Token::Comma, 1),
            (b'=', _) => {
                return Err(format!("`=` at {start}: a comparison is `==`"));
            }
            _ => {
                let shown = text[start..].chars().next().unwrap_or(' ');
                return Err(format!("`{shown}` at {start} is not part of an expression"));
            }
        };
        read.push((token, start));
        at += width;
    }
    Ok(read)
}

struct Parser<'a> {
    tokens: Vec<Placed>,
    at: usize,
    end: usize,
    names: &'a [&'a str],
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    /// Where the next token starts, or the end of the text.
    fn here(&self) -> usize {
        self.tokens.get(self.at).map_or(self.end, |(_, at)| *at)
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MOST_DEPTH {
            return Err(format!(
                "nested more than {MOST_DEPTH} deep at {}",
                self.here()
            ));
        }
        Ok(())
    }

    /// An expression whose operators all bind tighter than `above`.
    fn expression(&mut self, above: u8) -> Result<Node, String> {
        self.deeper()?;
        let mut left = self.unary()?;
        loop {
            // A `-` after a value subtracts; before one it negates (`unary`).
            let op = match self.peek() {
                Some(Token::Op(op)) => *op,
                Some(Token::Minus) => Op::Subtract,
                _ => break,
            };
            let rank = op.rank();
            if rank <= above {
                break;
            }
            self.at += 1;
            // `^` groups to the right: its right side may hold another `^`.
            let right = self.expression(if op == Op::Power { rank - 1 } else { rank })?;
            left = Node::Binary(op, Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }

    fn unary(&mut self) -> Result<Node, String> {
        match self.peek() {
            Some(Token::Minus) => {
                self.at += 1;
                Ok(Node::Negate(Box::new(self.expression(UNARY)?)))
            }
            Some(Token::Not) => {
                self.at += 1;
                Ok(Node::Not(Box::new(self.expression(UNARY)?)))
            }
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<Node, String> {
        let at = self.here();
        let Some((token, _)) = self.tokens.get(self.at).cloned() else {
            return Err("it ends where a number or a name was expected".to_owned());
        };
        self.at += 1;
        match token {
            Token::Number(number) => Ok(Node::Number(number)),
            Token::Open => {
                let inner = self.expression(0)?;
                self.close(at)?;
                Ok(inner)
            }
            Token::Name(name) if self.peek() == Some(&Token::Open) => self.call(&name, at),
            Token::Name(name) => {
                if self.names.contains(&name.as_str()) {
                    return Ok(Node::Param(name));
                }
                let meant = nearest(&name, self.names)
                    .map(|meant| format!("; did you mean `{meant}`?"))
                    .unwrap_or_default();
                Err(format!("`{name}` at {at} is not a param{meant}"))
            }
            Token::Op(_) | Token::Not | Token::Minus | Token::Close | Token::Comma => {
                Err(format!("at {at}, a number, a name or `(` was expected"))
            }
        }
    }

    fn close(&mut self, opened: usize) -> Result<(), String> {
        if self.peek() == Some(&Token::Close) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("the `(` at {opened} is not closed"))
        }
    }

    fn call(&mut self, name: &str, at: usize) -> Result<Node, String> {
        let names: Vec<&str> = FUNCTIONS.iter().map(|(name, _, _)| *name).collect();
        let Some(which) = names.iter().position(|each| *each == name) else {
            let meant = nearest(name, &names)
                .map(|meant| format!("; did you mean `{meant}`?"))
                .unwrap_or_default();
            return Err(format!(
                "`{name}` at {at} is not a function: they are {}{meant}",
                names.join(", ")
            ));
        };
        let opened = self.here();
        self.at += 1;
        let mut arguments = Vec::new();
        if self.peek() != Some(&Token::Close) {
            loop {
                arguments.push(self.expression(0)?);
                if self.peek() == Some(&Token::Comma) {
                    self.at += 1;
                } else {
                    break;
                }
            }
        }
        self.close(opened)?;
        let (_, least, most) = FUNCTIONS[which];
        if arguments.len() < least || most.is_some_and(|most| arguments.len() > most) {
            let wanted = match most {
                Some(most) if most == least => format!("{least}"),
                Some(most) => format!("{least} to {most}"),
                None => format!("{least} or more"),
            };
            return Err(format!(
                "`{name}` at {at} takes {wanted} arguments, not {}",
                arguments.len()
            ));
        }
        Ok(Node::Call(which, arguments))
    }
}

impl Expr {
    /// `text` read as an expression whose names are among `names`, the
    /// effect's params: refused, with where and why, for anything else.
    /// `tests::a_name_that_is_no_param_is_refused_with_the_one_meant`.
    pub fn parse(text: &str, names: &[&str]) -> Result<Self, String> {
        if text.len() > MOST_BYTES {
            return Err(format!("longer than {MOST_BYTES} bytes"));
        }
        let mut parser = Parser {
            tokens: tokens(text)?,
            at: 0,
            end: text.len(),
            names,
            depth: 0,
        };
        let node = parser.expression(0)?;
        if parser.at < parser.tokens.len() {
            return Err(format!(
                "at {}, the expression should have ended",
                parser.here()
            ));
        }
        Ok(Self {
            node,
            text: text.to_owned(),
        })
    }

    /// The text it was read from.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Every param it reads, each once, in the order it reads them.
    #[must_use]
    pub fn params(&self) -> Vec<&str> {
        fn walk<'a>(node: &'a Node, into: &mut Vec<&'a str>) {
            match node {
                Node::Number(_) => {}
                Node::Param(name) => {
                    if !into.contains(&name.as_str()) {
                        into.push(name);
                    }
                }
                Node::Negate(inner) | Node::Not(inner) => walk(inner, into),
                Node::Binary(_, left, right) => {
                    walk(left, into);
                    walk(right, into);
                }
                Node::Call(_, arguments) => {
                    for argument in arguments {
                        walk(argument, into);
                    }
                }
            }
        }
        let mut read = Vec::new();
        walk(&self.node, &mut read);
        read
    }

    /// Its value with the params `values`: a param missing from them, or
    /// one that is not a number, is NaN.
    /// `tests::blurs_reach_is_what_its_lua_gave`.
    #[must_use]
    pub fn eval(&self, values: &[(String, Value)]) -> f64 {
        eval(&self.node, values)
    }
}

fn truth(yes: bool) -> f64 {
    if yes { 1.0 } else { 0.0 }
}

fn eval(node: &Node, values: &[(String, Value)]) -> f64 {
    match node {
        Node::Number(number) => *number,
        Node::Param(name) => match values.iter().find(|(each, _)| each == name) {
            Some((_, Value::Number(number))) => *number,
            #[expect(clippy::cast_precision_loss, reason = "a param, far below 2^53")]
            Some((_, Value::Int(int))) => *int as f64,
            Some((_, Value::Bool(yes))) => truth(*yes),
            Some((_, Value::Vec4(_) | Value::Word(_))) | None => f64::NAN,
        },
        Node::Negate(inner) => -eval(inner, values),
        Node::Not(inner) => truth(eval(inner, values) == 0.0),
        Node::Binary(op, left, right) => {
            let left = eval(left, values);
            // `and` and `or` read their right side only when it decides.
            match op {
                Op::And if left == 0.0 => return 0.0,
                Op::Or if left != 0.0 => return 1.0,
                _ => {}
            }
            let right = eval(right, values);
            match op {
                Op::And | Op::Or => truth(right != 0.0),
                Op::Less => truth(left < right),
                Op::LessEqual => truth(left <= right),
                Op::Greater => truth(left > right),
                Op::GreaterEqual => truth(left >= right),
                #[expect(
                    clippy::float_cmp,
                    reason = "`==` in an expression is exact, as in the Lua it replaces"
                )]
                Op::Equal => truth(left == right),
                #[expect(
                    clippy::float_cmp,
                    reason = "`~=` in an expression is exact, as in the Lua it replaces"
                )]
                Op::NotEqual => truth(left != right),
                Op::Add => left + right,
                Op::Subtract => left - right,
                Op::Multiply => left * right,
                Op::Divide => left / right,
                Op::Power => left.powf(right),
            }
        }
        Node::Call(which, arguments) => {
            let numbers: Vec<f64> = arguments
                .iter()
                .map(|argument| eval(argument, values))
                .collect();
            let first = numbers.first().copied().unwrap_or(f64::NAN);
            match (FUNCTIONS[*which].0, numbers.as_slice()) {
                ("min", _) => numbers.iter().copied().fold(first, f64::min),
                ("max", _) => numbers.iter().copied().fold(first, f64::max),
                ("clamp", [x, low, high]) => x.max(*low).min(*high),
                ("floor", _) => first.floor(),
                ("ceil", _) => first.ceil(),
                ("abs", _) => first.abs(),
                ("sqrt", _) => first.sqrt(),
                _ => f64::NAN,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Expr;
    use crate::spec::Value;

    fn value(text: &str, values: &[(&str, Value)]) -> f64 {
        let names: Vec<&str> = values.iter().map(|(name, _)| *name).collect();
        let values: Vec<(String, Value)> = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();
        Expr::parse(text, &names)
            .unwrap_or_else(|err| panic!("{text}: {err}"))
            .eval(&values)
    }

    fn plain(text: &str) -> f64 {
        value(text, &[])
    }

    /// Precedence is Lua's: `*` before `+`, `^` before a minus in front of
    /// it, and `^` grouping to the right.
    #[test]
    fn arithmetic_binds_as_lua_does() {
        assert!((plain("1 + 2 * 3") - 7.0).abs() < 1e-12);
        assert!((plain("(1 + 2) * 3") - 9.0).abs() < 1e-12);
        assert!((plain("2 ^ 3 ^ 2") - 512.0).abs() < 1e-12, "right to left");
        assert!((plain("-2 ^ 2") + 4.0).abs() < 1e-12, "the power first");
        assert!((plain("2 ^ -1") - 0.5).abs() < 1e-12);
        assert!((plain("10 - 4 - 3") - 3.0).abs() < 1e-12, "left to right");
        assert!((plain("12 / 4 / 3") - 1.0).abs() < 1e-12);
        assert!((plain("1.5e2 + .5") - 150.5).abs() < 1e-12);
    }

    /// Blur's `reach`, `ceil(offset * 2 ^ (passes + 1))`, gives what its
    /// `effect.lua`'s function gave: 48 at its defaults, 3 passes of 3.
    #[test]
    fn blurs_reach_is_what_its_lua_gave() {
        let reach = |passes: i64, offset: f64| {
            value(
                "ceil(offset * 2 ^ (passes + 1))",
                &[
                    ("offset", Value::Number(offset)),
                    ("passes", Value::Int(passes)),
                ],
            )
        };
        assert!((reach(3, 3.0) - 48.0).abs() < 1e-12);
        assert!((reach(1, 2.5) - 10.0).abs() < 1e-12);
        assert!((reach(6, 0.3) - 39.0).abs() < 1e-12, "38.4, rounded up");
    }

    /// A comparison is 1 or 0, `and` and `or` take anything but 0 as true,
    /// and a param that is `true` is 1.
    #[test]
    fn comparisons_and_logic_are_one_or_zero() {
        let with = |text: &str| {
            value(
                text,
                &[
                    ("soften_passes", Value::Int(3)),
                    ("on", Value::Bool(true)),
                    ("off", Value::Bool(false)),
                ],
            )
        };
        assert!((with("soften_passes >= 1") - 1.0).abs() < 1e-12);
        assert!(with("soften_passes < 1").abs() < 1e-12);
        assert!((with("on and soften_passes == 3") - 1.0).abs() < 1e-12);
        assert!(with("off and on").abs() < 1e-12);
        assert!((with("off or on") - 1.0).abs() < 1e-12);
        assert!((with("not off") - 1.0).abs() < 1e-12);
        assert!((with("soften_passes ~= 2 and soften_passes != 4") - 1.0).abs() < 1e-12);
        assert!(
            with("not soften_passes > 5").abs() < 1e-12,
            "not binds tighter: (not 3) > 5"
        );
        assert!((with("not (soften_passes > 5)") - 1.0).abs() < 1e-12);
        assert!((with("true + true") - 2.0).abs() < 1e-12);
    }

    #[test]
    fn functions_take_their_arguments() {
        assert!((plain("min(4, 2, 3)") - 2.0).abs() < 1e-12);
        assert!((plain("max(4, 2, 9)") - 9.0).abs() < 1e-12);
        assert!((plain("clamp(7, 0, 5)") - 5.0).abs() < 1e-12);
        assert!((plain("clamp(-1, 0, 5)")).abs() < 1e-12);
        assert!((plain("floor(2.7) + ceil(2.1) + abs(-1)") - 6.0).abs() < 1e-12);
        assert!((plain("sqrt(16)") - 4.0).abs() < 1e-12);
        for (text, said) in [
            ("clamp(1, 2)", "takes 3"),
            ("floor(1, 2)", "takes 1"),
            ("min()", "1 or more"),
            ("round(2.5)", "not a function"),
            ("sqr(4)", "`sqrt`"),
        ] {
            let refused = Expr::parse(text, &[]).expect_err(text);
            assert!(refused.contains(said), "{text}: {refused}");
        }
    }

    /// A misspelt param is refused where the effect is loaded, naming the
    /// param probably meant, and so is anything that is not an expression.
    #[test]
    fn a_name_that_is_no_param_is_refused_with_the_one_meant() {
        let refused = Expr::parse("pases + 1", &["passes", "offset"]).expect_err("a typo");
        assert!(
            refused.contains("`pases`") && refused.contains("`passes`"),
            "{refused}"
        );
        for (text, said) in [
            ("1 +", "ends"),
            ("(1 + 2", "not closed"),
            ("1 2", "should have ended"),
            ("a = 1", "`==`"),
            ("1 % 2", "`%`"),
            ("", "ends"),
            ("1..2", "not a number"),
        ] {
            let refused = Expr::parse(text, &["a"]).expect_err(text);
            assert!(refused.contains(said), "{text}: {refused}");
        }
    }

    /// Nothing panics: a division by 0 is an infinity or NaN, for whatever
    /// reads it to refuse, and a param that is no number is NaN.
    #[test]
    fn evaluation_is_total() {
        assert!(plain("1 / 0").is_infinite());
        assert!(plain("0 / 0").is_nan());
        assert!(plain("sqrt(-1)").is_nan());
        assert!(
            value("tint + 1", &[("tint", Value::Vec4([0.0; 4]))]).is_nan(),
            "four numbers are no number"
        );
        let parsed = Expr::parse("gone * 2", &["gone"]).expect("parses");
        assert!(parsed.eval(&[]).is_nan(), "a param not given");
    }

    /// An expression nested past the limit is refused, not recursed into,
    /// and one longer than the limit is not read at all.
    #[test]
    fn nesting_past_the_limit_is_refused_not_followed() {
        let deep = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
        assert!(
            Expr::parse(&deep, &[])
                .expect_err("too deep")
                .contains("longer")
        );
        let deep = format!("{}1{}", "(".repeat(100), ")".repeat(100));
        assert!(
            Expr::parse(&deep, &[])
                .expect_err("too deep")
                .contains("nested")
        );
        let minus = format!("{}1", "-".repeat(100));
        assert!(
            Expr::parse(&minus, &[])
                .expect_err("too deep")
                .contains("nested")
        );
        let fine = format!("{}1{}", "(".repeat(30), ")".repeat(30));
        assert!((plain(&fine) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn it_names_the_params_it_reads() {
        let parsed = Expr::parse("a + b * a + min(c, 1)", &["a", "b", "c", "d"]).expect("parses");
        assert_eq!(parsed.params(), vec!["a", "b", "c"]);
        assert_eq!(parsed.text(), "a + b * a + min(c, 1)");
    }
}
